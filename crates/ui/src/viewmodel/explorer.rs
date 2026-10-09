//! ViewModel: the data source explorer — one source's tables, read live from its database, and
//! the draft choice of which of them `pipa-ingestion` captures, saved as a whole.

use std::collections::BTreeSet;

use leptos::prelude::*;
use leptos::task::spawn_local;
use pipa_api::{DataSourceView, DbEngine, SourceTableRef, SourceTableView};

use crate::api;
use crate::viewmodel::StatusMessage;

/// Reactive state for the explorer page. Like every ViewModel it is a bundle of `RwSignal`
/// handles, cheap to `Copy`.
#[derive(Copy, Clone)]
pub struct ExplorerViewModel {
    pub source: RwSignal<Option<DataSourceView>>,
    pub tables: RwSignal<Vec<SourceTableView>>,
    /// The tables ticked on the page, not yet saved.
    pub chosen: RwSignal<BTreeSet<SourceTableRef>>,
    /// The tables the source ingests as last saved; `chosen` differs from it while there are
    /// unsaved changes.
    saved: RwSignal<BTreeSet<SourceTableRef>>,
    /// Narrows the list to tables whose `schema.name` contains it (case-insensitive).
    pub filter: RwSignal<String>,
    /// The tables whose columns are shown; every table starts collapsed.
    pub open: RwSignal<BTreeSet<SourceTableRef>>,
    /// The schemas whose tables are shown, when tables are grouped by schema; every schema
    /// starts collapsed.
    pub open_schemas: RwSignal<BTreeSet<String>>,
    pub loading: RwSignal<bool>,
    pub saving: RwSignal<bool>,
    /// Why the tables couldn't be read (e.g. the source's database is unreachable).
    pub error: RwSignal<Option<String>>,
    status: RwSignal<Option<StatusMessage>>,
}

impl ExplorerViewModel {
    pub fn new(status: RwSignal<Option<StatusMessage>>) -> Self {
        Self {
            source: RwSignal::new(None),
            tables: RwSignal::new(Vec::new()),
            chosen: RwSignal::new(BTreeSet::new()),
            saved: RwSignal::new(BTreeSet::new()),
            filter: RwSignal::new(String::new()),
            open: RwSignal::new(BTreeSet::new()),
            open_schemas: RwSignal::new(BTreeSet::new()),
            loading: RwSignal::new(false),
            saving: RwSignal::new(false),
            error: RwSignal::new(None),
            status,
        }
    }

    /// Loads source `id` and its tables, discarding any unsaved choice.
    pub fn load(&self, id: String) {
        let this = *self;
        this.loading.set(true);
        this.error.set(None);
        this.tables.set(Vec::new());
        this.open.set(BTreeSet::new());
        this.open_schemas.set(BTreeSet::new());
        spawn_local(async move {
            match api::get_source(&id).await {
                Ok(source) => this.source.set(Some(source)),
                Err(err) => {
                    this.source.set(None);
                    this.status.set(Some(StatusMessage::Error(format!(
                        "Failed to load data source: {err}"
                    ))));
                    this.loading.set(false);
                    return;
                }
            }
            match api::list_source_tables(&id).await {
                Ok(tables) => {
                    let ingested = ingested_of(&tables);
                    this.tables.set(tables);
                    this.saved.set(ingested.clone());
                    this.chosen.set(ingested);
                }
                Err(err) => this.error.set(Some(err)),
            }
            this.loading.set(false);
        });
    }

    /// Saved tables the source's database no longer has (dropped or renamed). They can't be
    /// ticked, since they aren't listed, so the next save drops them.
    pub fn missing_tables(&self) -> Vec<SourceTableRef> {
        let saved = self.source.with(|source| {
            source
                .as_ref()
                .map(|source| source.ingested_tables.clone())
                .unwrap_or_default()
        });
        self.tables.with(|tables| {
            saved
                .into_iter()
                .filter(|table| !tables.iter().any(|found| &table_ref(found) == table))
                .collect()
        })
    }

    /// The listed tables matching [`Self::filter`].
    pub fn visible_tables(&self) -> Vec<SourceTableView> {
        let filter = self.filter.get();
        self.tables.with(|tables| {
            tables
                .iter()
                .filter(|table| matches_filter(table, &filter))
                .cloned()
                .collect()
        })
    }

    /// Whether tables are listed under their schema: Postgres sources only, since a MySQL
    /// source's schema is its one database.
    pub fn groups_by_schema(&self) -> bool {
        self.source.with(|source| {
            source
                .as_ref()
                .is_some_and(|source| source.engine == DbEngine::Postgres)
        })
    }

    /// The schemas of the tables the filter shows, in order.
    pub fn visible_schemas(&self) -> Vec<String> {
        schemas_of(&self.visible_tables())
    }

    /// The tables of `schema` the filter shows.
    pub fn visible_tables_in(&self, schema: &str) -> Vec<SourceTableView> {
        self.visible_tables()
            .into_iter()
            .filter(|table| table.schema == schema)
            .collect()
    }

    /// `(chosen, shown)`: how many of the tables of `schema` the filter shows are ticked.
    pub fn schema_choice(&self, schema: &str) -> (usize, usize) {
        let tables = self.visible_tables_in(schema);
        let chosen = self.chosen.with(|chosen| {
            tables
                .iter()
                .filter(|table| chosen.contains(&table_ref(table)))
                .count()
        });
        (chosen, tables.len())
    }

    /// Ticks (or unticks) every table of `schema` the filter shows.
    pub fn set_schema_chosen(&self, schema: &str, chosen: bool) {
        let tables = self
            .visible_tables_in(schema)
            .iter()
            .map(table_ref)
            .collect();
        self.set_all_chosen(tables, chosen);
    }

    pub fn is_chosen(&self, table: &SourceTableRef) -> bool {
        self.chosen.with(|chosen| chosen.contains(table))
    }

    pub fn set_chosen(&self, table: SourceTableRef, chosen: bool) {
        self.chosen.update(|set| {
            if chosen {
                set.insert(table);
            } else {
                set.remove(&table);
            }
        });
    }

    /// Ticks (or, with `chosen == false`, unticks) every table the filter shows.
    pub fn set_all_visible(&self, chosen: bool) {
        let visible = self.visible_tables().iter().map(table_ref).collect();
        self.set_all_chosen(visible, chosen);
    }

    fn set_all_chosen(&self, tables: Vec<SourceTableRef>, chosen: bool) {
        self.chosen.update(|set| {
            for table in tables {
                if chosen {
                    set.insert(table);
                } else {
                    set.remove(&table);
                }
            }
        });
    }

    /// Whether the source ingests `table` as last saved.
    pub fn is_ingested(&self, table: &SourceTableRef) -> bool {
        self.saved.with(|saved| saved.contains(table))
    }

    pub fn chosen_count(&self) -> usize {
        self.chosen.with(BTreeSet::len)
    }

    pub fn is_dirty(&self) -> bool {
        self.chosen.get() != self.saved.get()
    }

    /// What saving would change: `(newly chosen, no longer chosen)`.
    pub fn pending_changes(&self) -> (usize, usize) {
        self.chosen.with(|chosen| {
            self.saved.with(|saved| {
                (
                    chosen.difference(saved).count(),
                    saved.difference(chosen).count(),
                )
            })
        })
    }

    pub fn discard(&self) {
        self.chosen.set(self.saved.get_untracked());
    }

    pub fn is_open(&self, table: &SourceTableRef) -> bool {
        self.open.with(|open| open.contains(table))
    }

    pub fn toggle_open(&self, table: SourceTableRef) {
        self.open.update(|open| {
            if !open.remove(&table) {
                open.insert(table);
            }
        });
    }

    /// Whether every table the filter shows is expanded (and there is at least one).
    /// Also requires every shown schema to be open, when tables are grouped by schema.
    pub fn all_visible_open(&self) -> bool {
        let tables = self.visible_tables();
        let schemas_open = !self.groups_by_schema()
            || self
                .open_schemas
                .with(|open| tables.iter().all(|table| open.contains(&table.schema)));
        !tables.is_empty()
            && schemas_open
            && self
                .open
                .with(|open| tables.iter().all(|table| open.contains(&table_ref(table))))
    }

    /// Expands every table the filter shows, and their schemas, or collapses them all when they
    /// already are.
    pub fn toggle_all_visible_open(&self) {
        let expand = !self.all_visible_open();
        let tables = self.visible_tables();
        self.open.update(|open| {
            for table in tables.iter().map(table_ref) {
                if expand {
                    open.insert(table);
                } else {
                    open.remove(&table);
                }
            }
        });
        self.open_schemas.update(|open| {
            for schema in schemas_of(&tables) {
                if expand {
                    open.insert(schema);
                } else {
                    open.remove(&schema);
                }
            }
        });
    }

    pub fn is_schema_open(&self, schema: &str) -> bool {
        self.open_schemas.with(|open| open.contains(schema))
    }

    /// Folds `schema`'s tables away or shows them again; each table keeps its own state.
    pub fn toggle_schema(&self, schema: &str) {
        self.open_schemas.update(|open| {
            if !open.remove(schema) {
                open.insert(schema.to_string());
            }
        });
    }

    pub fn save(&self) {
        let Some(id) = self
            .source
            .with_untracked(|source| source.as_ref().map(|s| s.id.clone()))
        else {
            return;
        };
        let this = *self;
        let tables: Vec<SourceTableRef> = this.chosen.get_untracked().into_iter().collect();
        this.saving.set(true);
        spawn_local(async move {
            match api::set_ingested_tables(&id, tables).await {
                Ok(source) => {
                    let saved: BTreeSet<SourceTableRef> =
                        source.ingested_tables.iter().cloned().collect();
                    this.saved.set(saved.clone());
                    this.chosen.set(saved);
                    this.source.set(Some(source));
                    this.status.set(Some(StatusMessage::Success(
                        "Ingested tables saved. Ingestion picks them up within a minute."
                            .to_string(),
                    )));
                }
                Err(err) => this.status.set(Some(StatusMessage::Error(format!(
                    "Failed to save ingested tables: {err}"
                )))),
            }
            this.saving.set(false);
        });
    }
}

pub fn table_ref(table: &SourceTableView) -> SourceTableRef {
    SourceTableRef {
        schema: table.schema.clone(),
        name: table.name.clone(),
    }
}

/// The tables the backend flagged as ingested.
fn ingested_of(tables: &[SourceTableView]) -> BTreeSet<SourceTableRef> {
    tables
        .iter()
        .filter(|table| table.ingested)
        .map(table_ref)
        .collect()
}

/// The distinct schemas of `tables`, in the order they first appear (the backend lists tables
/// sorted by schema).
fn schemas_of(tables: &[SourceTableView]) -> Vec<String> {
    let mut schemas: Vec<String> = Vec::new();
    for table in tables {
        if !schemas.contains(&table.schema) {
            schemas.push(table.schema.clone());
        }
    }
    schemas
}

fn matches_filter(table: &SourceTableView, filter: &str) -> bool {
    let filter = filter.trim().to_lowercase();
    filter.is_empty()
        || format!("{}.{}", table.schema, table.name)
            .to_lowercase()
            .contains(&filter)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(schema: &str, name: &str, ingested: bool) -> SourceTableView {
        SourceTableView {
            schema: schema.to_string(),
            name: name.to_string(),
            columns: Vec::new(),
            ingested,
        }
    }

    #[test]
    fn filters_on_the_qualified_name_ignoring_case() {
        let orders = table("public", "Orders", false);
        assert!(matches_filter(&orders, ""));
        assert!(matches_filter(&orders, "  ord "));
        assert!(matches_filter(&orders, "public.o"));
        assert!(!matches_filter(&orders, "audit"));
    }

    #[test]
    fn lists_each_schema_once_in_order() {
        let schemas = schemas_of(&[
            table("fintech", "accounts", false),
            table("fintech", "cards", false),
            table("public", "orders", false),
        ]);
        assert_eq!(schemas, vec!["fintech".to_string(), "public".to_string()]);
    }

    #[test]
    fn starts_from_the_tables_flagged_as_ingested() {
        let chosen = ingested_of(&[
            table("public", "orders", true),
            table("public", "customers", false),
        ]);
        assert_eq!(
            chosen.into_iter().collect::<Vec<_>>(),
            vec![SourceTableRef {
                schema: "public".to_string(),
                name: "orders".to_string(),
            }]
        );
    }
}
