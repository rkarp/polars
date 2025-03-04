use crate::prelude::*;

pub struct ExprIter<'a> {
    stack: Vec<&'a Expr>,
}

impl<'a> Iterator for ExprIter<'a> {
    type Item = &'a Expr;

    fn next(&mut self) -> Option<Self::Item> {
        self.stack.pop().map(|current_expr| {
            use Expr::*;
            let mut push = |e: &'a Expr| self.stack.push(e);

            match current_expr {
                Column(_) | Literal(_) | Wildcard => {}
                Alias(e, _) => push(e),
                Not(e) => push(e),
                BinaryExpr { left, op: _, right } => {
                    push(left);
                    push(right);
                }
                IsNull(e) => push(e),
                IsNotNull(e) => push(e),
                Cast { expr, .. } => push(expr),
                Sort { expr, .. } => push(expr),
                Filter { input, by } => {
                    push(input);
                    push(by)
                }
                SortBy { expr, by, .. } => {
                    push(expr);
                    push(by)
                }
                Agg(agg_e) => {
                    use AggExpr::*;
                    match agg_e {
                        Max(e) => push(e),
                        Min(e) => push(e),
                        Mean(e) => push(e),
                        Median(e) => push(e),
                        NUnique(e) => push(e),
                        First(e) => push(e),
                        Last(e) => push(e),
                        List(e) => push(e),
                        Count(e) => push(e),
                        Quantile { expr, .. } => push(expr),
                        Sum(e) => push(e),
                        AggGroups(e) => push(e),
                        Std(e) => push(e),
                        Var(e) => push(e),
                    }
                }
                Ternary {
                    truthy,
                    falsy,
                    predicate,
                } => {
                    push(truthy);
                    push(falsy);
                    push(predicate)
                }
                Udf { input, .. } => push(input),
                Shift { input, .. } => push(input),
                Reverse(e) => push(e),
                Duplicated(e) => push(e),
                IsUnique(e) => push(e),
                Explode(e) => push(e),
                Window {
                    function,
                    partition_by,
                    order_by,
                } => {
                    push(function);
                    push(partition_by);
                    if let Some(e) = order_by {
                        push(e);
                    }
                }
                Slice { input, .. } => push(input),
                BinaryFunction {
                    input_a, input_b, ..
                } => {
                    push(input_a);
                    push(input_b)
                }
                Except(e) => push(e),
            }
            current_expr
        })
    }
}

impl<'a> IntoIterator for &'a Expr {
    type Item = &'a Expr;
    type IntoIter = ExprIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        let mut stack = Vec::with_capacity(8);
        stack.push(self);
        ExprIter { stack }
    }
}

impl AExpr {
    /// Push nodes at this level to a pre-allocated stack
    pub(crate) fn nodes<'a>(&'a self, container: &mut Vec<Node>) {
        let mut push = |e: &'a Node| container.push(*e);
        use AExpr::*;

        match self {
            Column(_) | Literal(_) | Wildcard => {}
            Alias(e, _) => push(e),
            Not(e) => push(e),
            BinaryExpr { left, op: _, right } => {
                push(left);
                push(right);
            }
            IsNull(e) => push(e),
            IsNotNull(e) => push(e),
            Cast { expr, .. } => push(expr),
            Sort { expr, .. } => push(expr),
            SortBy { expr, by, .. } => {
                push(expr);
                push(by);
            }
            Filter { input, by } => {
                push(input);
                push(by);
            }
            Agg(agg_e) => {
                use AAggExpr::*;
                match agg_e {
                    Max(e) => push(e),
                    Min(e) => push(e),
                    Mean(e) => push(e),
                    Median(e) => push(e),
                    NUnique(e) => push(e),
                    First(e) => push(e),
                    Last(e) => push(e),
                    List(e) => push(e),
                    Count(e) => push(e),
                    Quantile { expr, .. } => push(expr),
                    Sum(e) => push(e),
                    AggGroups(e) => push(e),
                    Std(e) => push(e),
                    Var(e) => push(e),
                }
            }
            Ternary {
                truthy,
                falsy,
                predicate,
            } => {
                push(truthy);
                push(falsy);
                push(predicate)
            }
            Udf { input, .. } => push(input),
            Shift { input, .. } => push(input),
            Reverse(e) => push(e),
            Duplicated(e) => push(e),
            IsUnique(e) => push(e),
            Explode(e) => push(e),
            Window {
                function,
                partition_by,
                order_by,
            } => {
                push(function);
                push(partition_by);
                if let Some(e) = order_by {
                    push(e);
                }
            }
            Slice { input, .. } => push(input),
            BinaryFunction {
                input_a, input_b, ..
            } => {
                push(input_a);
                push(input_b)
            }
            Except(input) => push(input),
        }
    }
}

pub struct AExprIter<'a> {
    stack: Vec<Node>,
    arena: Option<&'a Arena<AExpr>>,
}

impl<'a> Iterator for AExprIter<'a> {
    type Item = (Node, &'a AExpr);

    fn next(&mut self) -> Option<Self::Item> {
        self.stack.pop().map(|node| {
            // take the arena because the bchk doesn't allow a mutable borrow to the field.
            let arena = self.arena.unwrap();
            let current_expr = arena.get(node);
            current_expr.nodes(&mut self.stack);

            self.arena = Some(arena);
            (node, current_expr)
        })
    }
}

pub(crate) trait ArenaExprIter<'a> {
    fn iter(&self, root: Node) -> AExprIter<'a>;
}

impl<'a> ArenaExprIter<'a> for &'a Arena<AExpr> {
    fn iter(&self, root: Node) -> AExprIter<'a> {
        let mut stack = Vec::with_capacity(8);
        stack.push(root);
        AExprIter {
            stack,
            arena: Some(self),
        }
    }
}

pub struct AlpIter<'a> {
    stack: Vec<Node>,
    arena: &'a Arena<ALogicalPlan>,
}

pub(crate) trait ArenaLpIter<'a> {
    fn iter(&self, root: Node) -> AlpIter<'a>;
}

impl<'a> ArenaLpIter<'a> for &'a Arena<ALogicalPlan> {
    fn iter(&self, root: Node) -> AlpIter<'a> {
        let stack = vec![root];
        AlpIter { stack, arena: self }
    }
}

impl<'a> Iterator for AlpIter<'a> {
    type Item = (Node, &'a ALogicalPlan);

    fn next(&mut self) -> Option<Self::Item> {
        self.stack.pop().map(|node| {
            let lp = self.arena.get(node);
            lp.copy_inputs(&mut self.stack);
            (node, lp)
        })
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::prelude::*;
    use polars_core::df;
    use polars_core::prelude::*;

    #[test]
    fn test_lp_iter() -> Result<()> {
        let df = df! {
            "a" => [1, 2]
        }?;

        let (root, lp_arena, expr_arena) = df
            .lazy()
            .sort("a", false)
            .groupby(vec![col("a")])
            .agg(vec![col("a").first()])
            .logical_plan
            .into_alp();

        let cnt = (&lp_arena).iter(root).count();
        assert_eq!(cnt, 3);
        Ok(())
    }
}
