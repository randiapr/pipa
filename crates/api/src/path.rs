//! Route paths. The `const`s are the patterns the backend registers (axum's `{id}` syntax); the
//! functions build the concrete path a client requests, so the two can't be spelled differently.

pub const DATASOURCES: &str = "/datasources";
pub const DATASOURCE: &str = "/datasources/{id}";
pub const DATASOURCE_TEST: &str = "/datasources/{id}/test";
pub const PROJECTS: &str = "/projects";
pub const PROJECT: &str = "/projects/{id}";
pub const QUERY: &str = "/query";

fn with_id(pattern: &str, id: &str) -> String {
    pattern.replace("{id}", id)
}

pub fn datasource(id: &str) -> String {
    with_id(DATASOURCE, id)
}

pub fn datasource_test(id: &str) -> String {
    with_id(DATASOURCE_TEST, id)
}

pub fn project(id: &str) -> String {
    with_id(PROJECT, id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitutes_id_into_pattern() {
        assert_eq!(datasource("abc"), "/datasources/abc");
        assert_eq!(datasource_test("abc"), "/datasources/abc/test");
        assert_eq!(project("abc"), "/projects/abc");
    }
}
