use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Opaque cursor for keyset pagination, encoding the `(created_at, id)` of
/// the last row seen. Using both columns (rather than `created_at` alone)
/// keeps the cursor stable when multiple rows share the same timestamp.
#[derive(Debug, Clone, Copy)]
pub struct Cursor {
    pub created_at: DateTime<Utc>,
    pub id: Uuid,
}

impl Cursor {
    pub fn encode(&self) -> String {
        format!("{}_{}", self.created_at.to_rfc3339(), self.id)
    }

    pub fn decode(raw: &str) -> Option<Cursor> {
        let (ts, id) = raw.rsplit_once('_')?;
        let created_at = DateTime::parse_from_rfc3339(ts).ok()?.with_timezone(&Utc);
        let id = Uuid::parse_str(id).ok()?;
        Some(Cursor { created_at, id })
    }
}

/// Wraps a page of results with the cursor to request the next page.
/// `next_cursor` is `None` once the caller has reached the end of the set.
#[derive(serde::Serialize)]
pub struct Page<T> {
    pub data: Vec<T>,
    pub next_cursor: Option<String>,
}

impl<T> Page<T> {
    pub fn new(mut data: Vec<T>, limit: i64, cursor_of: impl Fn(&T) -> Cursor) -> Page<T> {
        let next_cursor = if data.len() as i64 > limit {
            data.pop();
            data.last().map(|last| cursor_of(last).encode())
        } else {
            None
        };
        Page { data, next_cursor }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use std::collections::HashSet;

    /// Minimal row shape mirroring what the repositories paginate over.
    #[derive(Debug, Clone)]
    struct Row {
        created_at: DateTime<Utc>,
        id: Uuid,
    }

    impl Row {
        fn cursor(&self) -> Cursor {
            Cursor {
                created_at: self.created_at,
                id: self.id,
            }
        }
    }

    /// Builds 25 rows in descending `(created_at, id)` order, the same
    /// ordering the keyset queries use. Several rows share a timestamp to
    /// exercise the tie-breaker on `id`.
    fn seed_rows() -> Vec<Row> {
        let base = Utc::now();
        let mut rows: Vec<Row> = (0..25)
            .map(|i| Row {
                // Groups of 5 share a timestamp.
                created_at: base - Duration::seconds((i / 5) as i64),
                id: Uuid::new_v4(),
            })
            .collect();
        rows.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.id.cmp(&a.id))
        });
        rows
    }

    /// Emulates the keyset query: return rows strictly after `cursor` in
    /// descending order, fetching `limit + 1` to detect a following page.
    fn fetch_page(rows: &[Row], cursor: Option<Cursor>, limit: i64) -> Page<Row> {
        let start = match cursor {
            None => 0,
            Some(c) => rows
                .iter()
                .position(|r| {
                    r.created_at < c.created_at
                        || (r.created_at == c.created_at && r.id < c.id)
                })
                .unwrap_or(rows.len()),
        };
        let window: Vec<Row> = rows
            .iter()
            .skip(start)
            .take((limit + 1) as usize)
            .cloned()
            .collect();
        Page::new(window, limit, |r| r.cursor())
    }

    fn assert_paginates_correctly(rows: Vec<Row>) {
        let limit = 10;

        // Page 1: 10 items, non-null cursor.
        let page1 = fetch_page(&rows, None, limit);
        assert_eq!(page1.data.len(), 10, "page 1 should have 10 items");
        let cursor1 = page1.next_cursor.expect("page 1 should have a next cursor");

        // Page 2: 10 items, different from page 1, non-null cursor.
        let page2 = fetch_page(&rows, Cursor::decode(&cursor1), limit);
        assert_eq!(page2.data.len(), 10, "page 2 should have 10 items");
        let cursor2 = page2.next_cursor.expect("page 2 should have a next cursor");
        let page1_ids: HashSet<Uuid> = page1.data.iter().map(|r| r.id).collect();
        assert!(
            page2.data.iter().all(|r| !page1_ids.contains(&r.id)),
            "page 2 must not repeat page 1 rows"
        );

        // Page 3: 5 items, null cursor (end of set).
        let page3 = fetch_page(&rows, Cursor::decode(&cursor2), limit);
        assert_eq!(page3.data.len(), 5, "page 3 should have 5 items");
        assert!(page3.next_cursor.is_none(), "page 3 should be the last page");

        // No duplicates or skips across all pages.
        let mut all_ids: Vec<Uuid> = Vec::new();
        all_ids.extend(page1.data.iter().map(|r| r.id));
        all_ids.extend(page2.data.iter().map(|r| r.id));
        all_ids.extend(page3.data.iter().map(|r| r.id));
        let unique: HashSet<Uuid> = all_ids.iter().copied().collect();
        assert_eq!(all_ids.len(), 25, "all 25 rows should be returned");
        assert_eq!(unique.len(), 25, "no row should be duplicated");
    }

    #[test]
    fn cursor_pagination_transactions() {
        assert_paginates_correctly(seed_rows());
    }

    #[test]
    fn cursor_pagination_withdrawals() {
        assert_paginates_correctly(seed_rows());
    }

    #[test]
    fn cursor_pagination_payment_requests() {
        assert_paginates_correctly(seed_rows());
    }

    #[test]
    fn cursor_roundtrip_preserves_position() {
        let row = seed_rows().into_iter().next().unwrap();
        let decoded = Cursor::decode(&row.cursor().encode()).expect("cursor should decode");
        assert_eq!(decoded.created_at, row.created_at);
        assert_eq!(decoded.id, row.id);
    }
}
