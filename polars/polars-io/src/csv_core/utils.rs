use crate::csv::CsvEncoding;
use crate::csv_core::parser::next_line_position;
use ahash::RandomState;
use lazy_static::lazy_static;
use polars_core::prelude::*;
use regex::{Regex, RegexBuilder};
use std::borrow::Cow;
use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom};

pub(crate) fn init_csv_reader<R: Read>(
    reader: R,
    has_header: bool,
    delimiter: u8,
) -> csv::Reader<R> {
    let mut reader_builder = csv::ReaderBuilder::new();
    reader_builder.has_headers(has_header);
    reader_builder.delimiter(delimiter);
    reader_builder.from_reader(reader)
}

pub(crate) fn get_file_chunks(
    bytes: &[u8],
    n_threads: usize,
    expected_fields: usize,
    delimiter: u8,
) -> Vec<(usize, usize)> {
    let mut last_pos = 0;
    let total_len = bytes.len();
    let chunk_size = total_len / n_threads;
    let mut offsets = Vec::with_capacity(n_threads);
    for _ in 0..n_threads {
        let search_pos = last_pos + chunk_size;

        if search_pos >= bytes.len() {
            break;
        }

        let end_pos = match next_line_position(&bytes[search_pos..], expected_fields, delimiter) {
            Some(pos) => search_pos + pos,
            None => {
                break;
            }
        };
        offsets.push((last_pos, end_pos + 1));
        last_pos = end_pos;
    }
    offsets.push((last_pos, total_len));
    offsets
}

lazy_static! {
    static ref DECIMAL_RE: Regex = Regex::new(r"^\s*-?(\d+\.\d+)$").unwrap();
    static ref INTEGER_RE: Regex = Regex::new(r"^\s*-?(\d+)$").unwrap();
    static ref BOOLEAN_RE: Regex = RegexBuilder::new(r"^\s*(true)$|^(false)$")
        .case_insensitive(true)
        .build()
        .unwrap();
}

/// Infer the data type of a record
fn infer_field_schema(string: &str) -> DataType {
    // when quoting is enabled in the reader, these quotes aren't escaped, we default to
    // Utf8 for them
    if string.starts_with('"') {
        return DataType::Utf8;
    }
    // match regex in a particular order
    if BOOLEAN_RE.is_match(string) {
        DataType::Boolean
    } else if DECIMAL_RE.is_match(string) {
        DataType::Float64
    } else if INTEGER_RE.is_match(string) {
        DataType::Int64
    } else {
        DataType::Utf8
    }
}

#[inline]
pub(crate) fn parse_bytes_with_encoding(bytes: &[u8], encoding: CsvEncoding) -> Result<Cow<str>> {
    let s = match encoding {
        CsvEncoding::Utf8 => std::str::from_utf8(bytes)
            .map_err(anyhow::Error::from)?
            .into(),
        CsvEncoding::LossyUtf8 => String::from_utf8_lossy(bytes),
    };
    Ok(s)
}

/// Infer the schema of a CSV file by reading through the first n records of the file,
/// with `max_read_records` controlling the maximum number of records to read.
///
/// If `max_read_records` is not set, the whole file is read to infer its schema.
///
/// Return inferred schema and number of records used for inference.
pub fn infer_file_schema<R: Read + Seek>(
    reader: &mut R,
    delimiter: u8,
    max_read_records: Option<usize>,
    has_header: bool,
    schema_overwrite: Option<&Schema>,
) -> Result<(Schema, usize)> {
    // We use lossy utf8 here because we don't want the schema inference to fail on utf8.
    // It may later.
    let encoding = CsvEncoding::LossyUtf8;
    // set headers to false otherwise the csv crate, skips them.
    let csv_reader = init_csv_reader(reader, false, delimiter);

    let mut records = csv_reader.into_byte_records();
    let header_length;

    // get or create header names
    // when has_header is false, creates default column names with column_ prefix
    let headers: Vec<String> = if let Some(byterecord) = records.next() {
        let byterecord = byterecord.map_err(anyhow::Error::from)?;
        header_length = byterecord.len();
        if has_header {
            byterecord
                .iter()
                .map(|slice| {
                    let s = parse_bytes_with_encoding(slice, encoding)?;
                    Ok(s.into())
                })
                .collect::<Result<_>>()?
        } else {
            (0..header_length)
                .map(|i| format!("column_{}", i + 1))
                .collect()
        }
    } else {
        return Err(PolarsError::NoData("empty csv".into()));
    };

    // keep track of inferred field types
    let mut column_types: Vec<HashSet<DataType, RandomState>> =
        vec![HashSet::with_hasher(RandomState::new()); header_length];
    // keep track of columns with nulls
    let mut nulls: Vec<bool> = vec![false; header_length];

    let mut records_count = 0;
    let mut fields = Vec::with_capacity(header_length);

    // needed to prevent ownership going into the iterator loop
    let records_ref = &mut records;

    for result in records_ref.take(max_read_records.unwrap_or(std::usize::MAX)) {
        let record = result.map_err(anyhow::Error::from)?;
        records_count += 1;

        for i in 0..header_length {
            if let Some(slice) = record.get(i) {
                if slice.is_empty() {
                    nulls[i] = true;
                } else {
                    let s = parse_bytes_with_encoding(slice, encoding)?;
                    column_types[i].insert(infer_field_schema(&s));
                }
            }
        }
    }

    // build schema from inference results
    for i in 0..header_length {
        let possibilities = &column_types[i];
        let field_name = &headers[i];

        if let Some(schema_overwrite) = schema_overwrite {
            if let Ok(field_ovw) = schema_overwrite.field_with_name(field_name) {
                fields.push(field_ovw.clone());
                continue;
            }
        }

        // determine data type based on possible types
        // if there are incompatible types, use DataType::Utf8
        match possibilities.len() {
            1 => {
                for dtype in possibilities.iter() {
                    fields.push(Field::new(&field_name, dtype.clone()));
                }
            }
            2 => {
                if possibilities.contains(&DataType::Int64)
                    && possibilities.contains(&DataType::Float64)
                {
                    // we have an integer and double, fall down to double
                    fields.push(Field::new(&field_name, DataType::Float64));
                } else {
                    // default to Utf8 for conflicting datatypes (e.g bool and int)
                    fields.push(Field::new(&field_name, DataType::Utf8));
                }
            }
            _ => fields.push(Field::new(&field_name, DataType::Utf8)),
        }
    }
    let csv_reader = records.into_reader();

    // return the reader seek back to the start
    csv_reader.into_inner().seek(SeekFrom::Start(0))?;

    Ok((Schema::new(fields), records_count))
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_get_file_chunks() {
        let path = "../../examples/aggregate_multiple_files_in_chunks/datasets/foods1.csv";
        let s = std::fs::read_to_string(path).unwrap();
        let bytes = s.as_bytes();
        // can be within -1 / +1 bounds.
        assert!((get_file_chunks(bytes, 10, 4, b',').len() as i32 - 10).abs() <= 1);
        assert!((get_file_chunks(bytes, 8, 4, b',').len() as i32 - 8).abs() <= 1);
    }
}
