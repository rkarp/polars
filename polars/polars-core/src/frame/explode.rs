use crate::chunked_array::ops::explode::offsets_to_indexes;
use crate::frame::select::Selection;
use crate::prelude::*;
use std::collections::VecDeque;

fn get_exploded(series: &Series) -> Result<(Series, &[i64])> {
    match series.dtype() {
        DataType::List(_) => series.list().unwrap().explode_and_offsets(),
        DataType::Utf8 => series.utf8().unwrap().explode_and_offsets(),
        _ => Err(PolarsError::InvalidOperation("".into())),
    }
}

impl DataFrame {
    /// Explode `DataFrame` to long format by exploding a column with Lists.
    ///
    /// # Example
    ///
    /// ```rust
    ///  use polars_core::prelude::*;
    ///  let s0 = Series::new("a", &[1i8, 2, 3]);
    ///  let s1 = Series::new("b", &[1i8, 1, 1]);
    ///  let s2 = Series::new("c", &[2i8, 2, 2]);
    ///  let list = Series::new("foo", &[s0, s1, s2]);
    ///
    ///  let s0 = Series::new("B", [1, 2, 3]);
    ///  let s1 = Series::new("C", [1, 1, 1]);
    ///  let df = DataFrame::new(vec![list, s0, s1]).unwrap();
    ///  let exploded = df.explode("foo").unwrap();
    ///
    ///  println!("{:?}", df);
    ///  println!("{:?}", exploded);
    /// ```
    /// Outputs:
    ///
    /// ```text
    ///  +-------------+-----+-----+
    ///  | foo         | B   | C   |
    ///  | ---         | --- | --- |
    ///  | list [i8]   | i32 | i32 |
    ///  +=============+=====+=====+
    ///  | "[1, 2, 3]" | 1   | 1   |
    ///  +-------------+-----+-----+
    ///  | "[1, 1, 1]" | 2   | 1   |
    ///  +-------------+-----+-----+
    ///  | "[2, 2, 2]" | 3   | 1   |
    ///  +-------------+-----+-----+
    ///
    ///  +-----+-----+-----+
    ///  | foo | B   | C   |
    ///  | --- | --- | --- |
    ///  | i8  | i32 | i32 |
    ///  +=====+=====+=====+
    ///  | 1   | 1   | 1   |
    ///  +-----+-----+-----+
    ///  | 2   | 1   | 1   |
    ///  +-----+-----+-----+
    ///  | 3   | 1   | 1   |
    ///  +-----+-----+-----+
    ///  | 1   | 2   | 1   |
    ///  +-----+-----+-----+
    ///  | 1   | 2   | 1   |
    ///  +-----+-----+-----+
    ///  | 1   | 2   | 1   |
    ///  +-----+-----+-----+
    ///  | 2   | 3   | 1   |
    ///  +-----+-----+-----+
    ///  | 2   | 3   | 1   |
    ///  +-----+-----+-----+
    ///  | 2   | 3   | 1   |
    ///  +-----+-----+-----+
    /// ```
    pub fn explode<'a, J, S: Selection<'a, J>>(&self, columns: S) -> Result<DataFrame> {
        let columns = self.select_series(columns)?;

        // first remove all the exploded columns
        let mut df = self.clone();
        for s in &columns {
            df = df.drop(s.name())?;
        }

        for (i, s) in columns.iter().enumerate() {
            if let Ok((exploded, offsets)) = get_exploded(s) {
                let col_idx = self.name_to_idx(s.name())?;

                // expand all the other columns based the exploded first column
                if i == 0 {
                    let row_idx = offsets_to_indexes(offsets, exploded.len());
                    df = unsafe { df.take_iter_unchecked(row_idx.into_iter()) };
                }
                if exploded.len() == df.height() {
                    df.columns.insert(col_idx, exploded);
                } else {
                    return Err(PolarsError::ShapeMisMatch(
                        format!("The exploded columns don't have the same length. Length DataFrame: {}. Length exploded column {}: {}", df.height(), exploded.name(), exploded.len()).into(),
                    ));
                }
            } else {
                return Err(PolarsError::InvalidOperation(
                    format!("cannot explode dtype: {:?}", s.dtype()).into(),
                ));
            }
        }
        Ok(df)
    }

    ///
    /// Unpivot a `DataFrame` from wide to long format.
    ///
    /// # Example
    ///
    /// # Arguments
    ///
    /// * `id_vars` - String slice that represent the columns to use as id variables.
    /// * `value_vars` - String slice that represent the columns to use as value variables.
    ///
    /// ```rust
    ///
    ///  # #[macro_use] extern crate polars_core;
    /// use polars_core::prelude::*;
    /// let df = df!("A" => &["a", "b", "a"],
    ///              "B" => &[1, 3, 5],
    ///              "C" => &[10, 11, 12],
    ///              "D" => &[2, 4, 6]
    ///     )
    /// .unwrap();
    ///
    /// let melted = df.melt(&["A", "B"], &["C", "D"]).unwrap();
    /// println!("{:?}", df);
    /// println!("{:?}", melted);
    /// ```
    /// Outputs:
    /// ```text
    ///  +-----+-----+-----+-----+
    ///  | A   | B   | C   | D   |
    ///  | --- | --- | --- | --- |
    ///  | str | i32 | i32 | i32 |
    ///  +=====+=====+=====+=====+
    ///  | "a" | 1   | 10  | 2   |
    ///  +-----+-----+-----+-----+
    ///  | "b" | 3   | 11  | 4   |
    ///  +-----+-----+-----+-----+
    ///  | "a" | 5   | 12  | 6   |
    ///  +-----+-----+-----+-----+
    ///
    ///  +-----+-----+----------+-------+
    ///  | A   | B   | variable | value |
    ///  | --- | --- | ---      | ---   |
    ///  | str | i32 | str      | i32   |
    ///  +=====+=====+==========+=======+
    ///  | "a" | 1   | "C"      | 10    |
    ///  +-----+-----+----------+-------+
    ///  | "b" | 3   | "C"      | 11    |
    ///  +-----+-----+----------+-------+
    ///  | "a" | 5   | "C"      | 12    |
    ///  +-----+-----+----------+-------+
    ///  | "a" | 1   | "D"      | 2     |
    ///  +-----+-----+----------+-------+
    ///  | "b" | 3   | "D"      | 4     |
    ///  +-----+-----+----------+-------+
    ///  | "a" | 5   | "D"      | 6     |
    ///  +-----+-----+----------+-------+
    /// ```
    pub fn melt<'a, 'b, J, K, SelId: Selection<'a, J>, SelValue: Selection<'b, K>>(
        &self,
        id_vars: SelId,
        value_vars: SelValue,
    ) -> Result<Self> {
        let ids = self.select(id_vars)?;
        let value_vars = value_vars.to_selection_vec();
        let len = self.height();

        let mut dataframe_chunks = VecDeque::with_capacity(value_vars.len());

        for value_column_name in value_vars {
            let variable_col = Utf8Chunked::full("variable", value_column_name, len).into_series();
            let mut value_col = self.column(value_column_name)?.clone();
            value_col.rename("value");

            let mut df_chunk = ids.clone();
            df_chunk.hstack_mut(&[variable_col, value_col])?;
            dataframe_chunks.push_back(df_chunk)
        }

        let mut main_df = dataframe_chunks
            .pop_front()
            .ok_or_else(|| PolarsError::NoData("No data in melt operation".into()))?;

        while let Some(df) = dataframe_chunks.pop_front() {
            main_df.vstack_mut(&df)?;
        }
        Ok(main_df)
    }
}

#[cfg(test)]
mod test {
    use crate::prelude::*;

    #[test]
    #[cfg(feature = "dtype-i8")]
    fn test_explode() {
        let s0 = Series::new("a", &[1i8, 2, 3]);
        let s1 = Series::new("b", &[1i8, 1, 1]);
        let s2 = Series::new("c", &[2i8, 2, 2]);
        let list = Series::new("foo", &[s0, s1, s2]);

        let s0 = Series::new("B", [1, 2, 3]);
        let s1 = Series::new("C", [1, 1, 1]);
        let df = DataFrame::new(vec![list, s0.clone(), s1.clone()]).unwrap();
        let exploded = df.explode("foo").unwrap();
        println!("{:?}", df);
        println!("{:?}", exploded);
        assert_eq!(exploded.shape(), (9, 3));
        assert_eq!(exploded.column("C").unwrap().i32().unwrap().get(8), Some(1));
        assert_eq!(exploded.column("B").unwrap().i32().unwrap().get(8), Some(3));
        assert_eq!(
            exploded.column("foo").unwrap().i8().unwrap().get(8),
            Some(2)
        );

        let str = Series::new("foo", &["abc", "de", "fg"]);
        let df = DataFrame::new(vec![str, s0, s1]).unwrap();
        let exploded = df.explode("foo").unwrap();
        println!("{:?}", df);
        println!("{:?}", exploded);
        assert_eq!(exploded.column("C").unwrap().i32().unwrap().get(6), Some(1));
        assert_eq!(exploded.column("B").unwrap().i32().unwrap().get(6), Some(3));
        assert_eq!(
            exploded.column("foo").unwrap().utf8().unwrap().get(6),
            Some("g")
        );
    }

    #[test]
    fn test_melt() {
        let df = df!("A" => &["a", "b", "a"],
         "B" => &[1, 3, 5],
         "C" => &[10, 11, 12],
         "D" => &[2, 4, 6]
        )
        .unwrap();

        let melted = df.melt(&["A", "B"], &["C", "D"]).unwrap();
        assert_eq!(
            Vec::from(melted.column("value").unwrap().i32().unwrap()),
            &[Some(10), Some(11), Some(12), Some(2), Some(4), Some(6)]
        )
    }
}
