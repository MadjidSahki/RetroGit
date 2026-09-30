//! Lane layout for the commit graph (pure: no I/O).

use crate::LogEntry;

/// One segment drawn inside a row: from column `from` to column `to`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edge {
    pub from: usize,
    pub to: usize,
    pub color: usize,
}

/// Drawing instructions for one commit row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphRow {
    /// Column of the commit dot.
    pub column: usize,
    pub color: usize,
    /// Top half: from the lanes above (`from`) to their position at the dot's height (`to`).
    pub up: Vec<Edge>,
    /// Bottom half: from the dot's height (`from`) to the lanes below (`to`).
    pub down: Vec<Edge>,
}

impl GraphRow {
    /// Number of columns this row touches.
    pub fn width(&self) -> usize {
        self.up
            .iter()
            .chain(&self.down)
            .map(|e| e.from.max(e.to) + 1)
            .chain([self.column + 1])
            .max()
            .unwrap_or(1)
    }
}

/// Lanes carried from one page of history to the next.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GraphState {
    /// For each column: the commit id this lane is waiting for, and its color.
    lanes: Vec<Option<(String, usize)>>,
    next_color: usize,
}

fn free_slot(lanes: &mut Vec<Option<(String, usize)>>, avoid: usize) -> usize {
    match lanes
        .iter()
        .enumerate()
        .position(|(i, l)| l.is_none() && i != avoid)
    {
        Some(i) => i,
        None => {
            lanes.push(None);
            lanes.len() - 1
        }
    }
}

impl GraphState {
    fn new_color(&mut self) -> usize {
        let c = self.next_color;
        self.next_color += 1;
        c
    }

    /// Lay out the next entries, continuing from the previous call.
    pub fn layout_more(&mut self, entries: &[LogEntry]) -> Vec<GraphRow> {
        entries.iter().map(|e| self.row(e)).collect()
    }

    fn row(&mut self, entry: &LogEntry) -> GraphRow {
        let before = self.lanes.clone();
        let waiting =
            |l: &Option<(String, usize)>| l.as_ref().is_some_and(|(id, _)| *id == entry.id);
        let found = before.iter().position(waiting);
        let (column, color) = match found {
            Some(c) => (c, before[c].as_ref().map_or(0, |l| l.1)),
            None => {
                let c = before
                    .iter()
                    .position(Option::is_none)
                    .unwrap_or(before.len());
                (c, self.new_color())
            }
        };
        let up = before
            .iter()
            .enumerate()
            .filter_map(|(i, l)| l.as_ref().map(|(id, col)| (i, id, *col)))
            .map(|(i, id, c)| Edge {
                from: i,
                to: if *id == entry.id { column } else { i },
                color: c,
            })
            .collect();

        // Lanes that were waiting for this commit end here.
        for l in self.lanes.iter_mut() {
            if waiting(l) {
                *l = None;
            }
        }
        if self.lanes.len() <= column {
            self.lanes.resize(column + 1, None);
        }
        let mut targets = Vec::new();
        for (k, parent) in entry.parents.iter().enumerate() {
            if k == 0 {
                self.lanes[column] = Some((parent.clone(), color));
                targets.push(column);
                continue;
            }
            let existing = self
                .lanes
                .iter()
                .position(|l| l.as_ref().is_some_and(|(id, _)| id == parent));
            let t = match existing {
                Some(t) => t,
                None => {
                    let t = free_slot(&mut self.lanes, column);
                    let c = self.new_color();
                    self.lanes[t] = Some((parent.clone(), c));
                    t
                }
            };
            targets.push(t);
        }
        while self.lanes.last().is_some_and(Option::is_none) {
            self.lanes.pop();
        }
        let down = self
            .lanes
            .iter()
            .enumerate()
            .filter_map(|(i, l)| l.as_ref().map(|(_, c)| (i, *c)))
            .map(|(i, c)| Edge {
                from: if targets.contains(&i) { column } else { i },
                to: i,
                color: c,
            })
            .collect();
        GraphRow {
            column,
            color,
            up,
            down,
        }
    }
}

/// Lay out a whole list at once (same result as successive `layout_more` calls).
pub fn layout(entries: &[LogEntry]) -> Vec<GraphRow> {
    GraphState::default().layout_more(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(id: &str, parents: &[&str]) -> LogEntry {
        LogEntry {
            id: id.into(),
            short_id: id.into(),
            parents: parents.iter().map(|p| p.to_string()).collect(),
            author: String::new(),
            email: String::new(),
            time: 0,
            summary: String::new(),
            refs: vec![],
        }
    }

    fn cols(rows: &[GraphRow]) -> Vec<usize> {
        rows.iter().map(|r| r.column).collect()
    }

    #[test]
    fn linear_history_stays_in_column_zero() {
        let rows = layout(&[e("c", &["b"]), e("b", &["a"]), e("a", &[])]);
        assert_eq!(cols(&rows), vec![0, 0, 0]);
        assert_eq!(rows[0].up, vec![]);
        assert_eq!(
            rows[0].down,
            vec![Edge {
                from: 0,
                to: 0,
                color: 0
            }]
        );
        assert_eq!(rows[2].down, vec![], "root: no line below");
        assert!(rows.iter().all(|r| r.color == 0));
    }

    #[test]
    fn merge_opens_a_second_lane_that_joins_back() {
        // m merges x (feature) into b; x and b both come from a.
        let rows = layout(&[
            e("m", &["b", "x"]),
            e("x", &["a"]),
            e("b", &["a"]),
            e("a", &[]),
        ]);
        assert_eq!(cols(&rows), vec![0, 1, 0, 0]);
        assert_eq!(
            rows[0].down,
            vec![
                Edge {
                    from: 0,
                    to: 0,
                    color: 0
                },
                Edge {
                    from: 0,
                    to: 1,
                    color: 1
                }
            ]
        );
        // x keeps lane 1 for its parent a; b continues lane 0 towards a too.
        assert_eq!(
            rows[1].down,
            vec![
                Edge {
                    from: 0,
                    to: 0,
                    color: 0
                },
                Edge {
                    from: 1,
                    to: 1,
                    color: 1
                }
            ]
        );
        // at a, lane 1 joins lane 0.
        assert_eq!(
            rows[3].up,
            vec![
                Edge {
                    from: 0,
                    to: 0,
                    color: 0
                },
                Edge {
                    from: 1,
                    to: 0,
                    color: 1
                }
            ]
        );
        assert_eq!(rows[3].down, vec![]);
    }

    #[test]
    fn parallel_branch_tips_get_their_own_lanes() {
        let rows = layout(&[e("t1", &["a"]), e("t2", &["a"]), e("a", &[])]);
        assert_eq!(cols(&rows), vec![0, 1, 0]);
        assert_ne!(rows[0].color, rows[1].color);
        assert_eq!(rows[2].up.len(), 2);
    }

    #[test]
    fn octopus_merge_and_multiple_roots() {
        let rows = layout(&[
            e("m", &["a", "b", "c"]),
            e("a", &[]),
            e("b", &[]),
            e("c", &[]),
        ]);
        assert_eq!(rows[0].down.len(), 3);
        assert_eq!(cols(&rows), vec![0, 0, 1, 2]);
        assert!(rows.iter().all(|r| r.width() <= 3));
    }

    #[test]
    fn freed_lanes_are_reused() {
        // x merged and gone, then y branches later and reuses column 1.
        let rows = layout(&[
            e("m2", &["m1", "y"]),
            e("y", &["m1"]),
            e("m1", &["b", "x"]),
            e("x", &["b"]),
            e("b", &[]),
        ]);
        assert_eq!(cols(&rows), vec![0, 1, 0, 1, 0]);
        assert!(rows.iter().all(|r| r.width() <= 2));
    }

    #[test]
    fn incremental_layout_matches_one_shot_layout() {
        let entries = vec![
            e("m2", &["m1", "y"]),
            e("y", &["m1"]),
            e("m1", &["b", "x"]),
            e("x", &["b"]),
            e("b", &["a"]),
            e("a", &[]),
        ];
        let mut state = GraphState::default();
        let mut paged = state.layout_more(&entries[..3]);
        paged.extend(state.layout_more(&entries[3..]));
        assert_eq!(paged, layout(&entries));
    }
}
