//! `project_groups`: sections of the project list. A project is in at most
//! one group (`projects.group_id`); deleting a group moves its projects to
//! "Not in a group" (the foreign key's ON DELETE SET NULL).

use rusqlite::{Connection, OptionalExtension, Row, params};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub id: i64,
    pub name: String,
    pub sort_order: i64,
    pub created_at: String,
}

const COLUMNS: &str = "id, name, sort_order, created_at";
/// The order of the project list's sections.
const ORDER: &str = "sort_order, name COLLATE NOCASE, id";

fn group(row: &Row<'_>) -> rusqlite::Result<Group> {
    Ok(Group {
        id: row.get(0)?,
        name: row.get(1)?,
        sort_order: row.get(2)?,
        created_at: row.get(3)?,
    })
}

/// Every group, in the list's order (sort order, then name).
pub fn list(conn: &Connection) -> rusqlite::Result<Vec<Group>> {
    conn.prepare(&format!(
        "SELECT {COLUMNS} FROM project_groups ORDER BY {ORDER}"
    ))?
    .query_map([], group)?
    .collect()
}

pub fn get(conn: &Connection, id: i64) -> rusqlite::Result<Option<Group>> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM project_groups WHERE id = ?1"),
        [id],
        group,
    )
    .optional()
}

/// Whether another group (not `except`) has `name`, ignoring case.
pub fn name_taken(conn: &Connection, name: &str, except: Option<i64>) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM project_groups WHERE name = ?1 COLLATE NOCASE AND id IS NOT ?2)",
        params![name, except],
        |r| r.get(0),
    )
}

/// Adds a group after the others and returns its id.
pub fn insert(conn: &Connection, name: &str) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO project_groups (name, sort_order)
         VALUES (?1, (SELECT COALESCE(MAX(sort_order), -1) + 1 FROM project_groups))",
        [name],
    )?;
    Ok(conn.last_insert_rowid())
}

/// `false` when there is no such group.
pub fn rename(conn: &Connection, id: i64, name: &str) -> rusqlite::Result<bool> {
    Ok(conn.execute(
        "UPDATE project_groups SET name = ?2 WHERE id = ?1",
        params![id, name],
    )? > 0)
}

/// Deletes a group; its projects stay, in no group. `false` when there is
/// no such group.
pub fn delete(conn: &Connection, id: i64) -> rusqlite::Result<bool> {
    Ok(conn.execute("DELETE FROM project_groups WHERE id = ?1", [id])? > 0)
}

/// Puts the groups in the order of `ids` (sort orders 0, 1, 2 …). `ids`
/// must name every group exactly once; `false` (and no change) otherwise.
pub fn set_order(conn: &mut Connection, ids: &[i64]) -> rusqlite::Result<bool> {
    let tx = conn.transaction()?;
    let mut current: Vec<i64> = list(&tx)?.iter().map(|g| g.id).collect();
    let mut wanted = ids.to_vec();
    current.sort_unstable();
    wanted.sort_unstable();
    if current != wanted {
        return Ok(false);
    }
    for (i, id) in ids.iter().enumerate() {
        tx.execute(
            "UPDATE project_groups SET sort_order = ?2 WHERE id = ?1",
            params![id, i as i64],
        )?;
    }
    tx.commit()?;
    Ok(true)
}

/// Moves a group one place up (`up`) or down in the list. `false` when there
/// is no such group; a group already first (or last) stays.
pub fn shift(conn: &mut Connection, id: i64, up: bool) -> rusqlite::Result<bool> {
    let mut ids: Vec<i64> = list(conn)?.iter().map(|g| g.id).collect();
    let Some(at) = ids.iter().position(|g| *g == id) else {
        return Ok(false);
    };
    let to = if up {
        at.checked_sub(1)
    } else {
        Some(at + 1).filter(|t| *t < ids.len())
    };
    if let Some(to) = to {
        ids.swap(at, to);
    }
    set_order(conn, &ids)
}

/// How many projects each group has: `(group id, projects)`.
pub fn project_counts(conn: &Connection) -> rusqlite::Result<Vec<(i64, i64)>> {
    conn.prepare(
        "SELECT group_id, COUNT(*) FROM projects WHERE group_id IS NOT NULL GROUP BY group_id",
    )?
    .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        crate::product::install();
        let mut c = Connection::open_in_memory().unwrap();
        c.execute_batch(vole_kit::db::CONNECTION_PRAGMAS).unwrap();
        vole_kit::db::apply_migrations(&mut c).unwrap();
        c
    }

    fn names(c: &Connection) -> Vec<String> {
        list(c).unwrap().into_iter().map(|g| g.name).collect()
    }

    #[test]
    fn groups_are_listed_by_sort_order_then_name() {
        let mut c = conn();
        let sylo = insert(&c, "Sylo").unwrap();
        let web = insert(&c, "Websites").unwrap();
        let apps = insert(&c, "apps").unwrap();
        assert_eq!(names(&c), ["Sylo", "Websites", "apps"]);
        c.execute("UPDATE project_groups SET sort_order = 0", [])
            .unwrap();
        assert_eq!(names(&c), ["apps", "Sylo", "Websites"]);

        assert!(set_order(&mut c, &[web, apps, sylo]).unwrap());
        assert_eq!(names(&c), ["Websites", "apps", "Sylo"]);
        assert!(
            !set_order(&mut c, &[web, apps]).unwrap(),
            "every group once"
        );
        assert!(!set_order(&mut c, &[web, apps, apps]).unwrap());
        assert_eq!(names(&c), ["Websites", "apps", "Sylo"]);

        assert!(shift(&mut c, sylo, true).unwrap());
        assert_eq!(names(&c), ["Websites", "Sylo", "apps"]);
        assert!(shift(&mut c, web, true).unwrap(), "already first: stays");
        assert!(shift(&mut c, web, false).unwrap());
        assert_eq!(names(&c), ["Sylo", "Websites", "apps"]);
        assert!(!shift(&mut c, 999, true).unwrap());
    }

    #[test]
    fn names_are_unique_ignoring_case() {
        let c = conn();
        let id = insert(&c, "Sylo").unwrap();
        assert!(name_taken(&c, "sylo", None).unwrap());
        assert!(!name_taken(&c, "SYLO", Some(id)).unwrap());
        assert!(insert(&c, "SYLO").is_err(), "the UNIQUE constraint holds");
        assert!(rename(&c, id, "Sylo apps").unwrap());
        assert!(!rename(&c, 999, "x").unwrap());
    }

    #[test]
    fn deleting_a_group_keeps_its_projects() {
        let mut c = conn();
        let group = insert(&c, "Sylo").unwrap();
        let project = crate::db::projects::insert(
            &mut c,
            "Sylo iOS",
            "sylo-ios",
            crate::project::Platform::Apple,
            "Default",
        )
        .unwrap();
        assert!(crate::db::projects::set_group(&c, project, Some(group)).unwrap());
        assert_eq!(project_counts(&c).unwrap(), [(group, 1)]);
        assert!(delete(&c, group).unwrap());
        let p = crate::db::projects::get(&c, project).unwrap().unwrap();
        assert_eq!(p.group_id, None, "in no group");
        assert!(!delete(&c, group).unwrap());
    }
}
