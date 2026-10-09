//! View: the data source explorer (`/sources/:id`). Lists the tables of one source's database,
//! read live from it, as [`Collapse`]s (one per table, its columns inside — like the Tables
//! page, collapses work at every width; only the columns inside go through `ResponsiveList`),
//! each with a checkbox choosing whether `pipa-ingestion` captures it. A Postgres source's
//! tables are grouped under a collapse per schema. The choice is saved as a whole, and saving
//! or discarding it asks for confirmation first ([`ConfirmDialog`]).

use leptos::prelude::*;
use leptos_router::hooks::use_params_map;
use pipa_api::{SourceColumnRef, SourceColumnView, SourceTableView};

use crate::components::{
    Alert, Badge, BadgeStyle, Card, Checkbox, Collapse, ConfirmDialog, ExpandToggle, Loading,
    TextInput, Tone, open_dialog,
};
use crate::view::{ResponsiveList, StatusToast, auto_dismiss};
use crate::viewmodel::{ExplorerViewModel, SourcesViewModel, StatusMessage, table_ref};

#[component]
pub fn Explorer() -> impl IntoView {
    let params = use_params_map();
    let status = RwSignal::new(Option::<StatusMessage>::None);
    let vm = ExplorerViewModel::new(status);

    Effect::new(move |_| {
        if let Some(id) = params.read().get("id") {
            vm.load(id);
        }
    });
    auto_dismiss(status);

    view! {
        <div class="flex flex-col gap-6">
            <div class="flex flex-col gap-1">
                <a class="link link-hover text-sm text-base-content/70" href="/sources">
                    "\u{2190} Data sources"
                </a>
                <h1 class="text-2xl font-bold break-all">
                    {move || {
                        vm.source
                            .get()
                            .map(|source| source.name)
                            .unwrap_or_else(|| "Data source".to_string())
                    }}
                </h1>
                {move || {
                    vm.source
                        .get()
                        .map(|source| {
                            let connection = SourcesViewModel::connection_label(&source);
                            view! {
                                <div class="flex flex-wrap items-center gap-2">
                                    <Badge tone=Tone::Ghost small=true>
                                        {SourcesViewModel::engine_label(source.engine)}
                                    </Badge>
                                    <span class="font-mono text-sm break-all text-base-content/70">
                                        {connection}
                                    </span>
                                </div>
                            }
                        })
                }}
                <p class="text-base-content/70">
                    "Choose the tables to capture into pipa. Changes to any other table are skipped, and a table chosen later is captured from then on, without its earlier history."
                </p>
            </div>

            <StatusToast status=status />

            {move || {
                let missing = vm.missing_tables();
                (!missing.is_empty())
                    .then(|| {
                        let names = missing
                            .iter()
                            .map(|table| format!("{}.{}", table.schema, table.name))
                            .collect::<Vec<_>>()
                            .join(", ");
                        view! {
                            <Alert tone=Tone::Warning soft=true>
                                {format!(
                                    "Chosen but no longer in the database: {names}. Saving drops them.",
                                )}
                            </Alert>
                        }
                    })
            }}
            {move || {
                vm.error
                    .get()
                    .map(|message| {
                        view! {
                            <Alert tone=Tone::Error>
                                {format!("Could not read the source's tables: {message}")}
                            </Alert>
                        }
                    })
            }}

            <Show when=move || vm.loading.get()>
                <Loading />
            </Show>

            <Show when=move || !vm.loading.get() && vm.error.get().is_none()>
                <Toolbar vm=vm />
                <Show
                    when=move || !vm.visible_tables().is_empty()
                    fallback=move || {
                        view! {
                            <p class="text-base-content/70">
                                {move || {
                                    if vm.tables.with(Vec::is_empty) {
                                        "The source's database has no tables."
                                    } else {
                                        "No table matches the filter."
                                    }
                                }}
                            </p>
                        }
                    }
                >
                    <Show
                        when=move || vm.groups_by_schema()
                        fallback=move || {
                            view! {
                                <div class="flex flex-col gap-2">
                                    <For
                                        each=move || vm.visible_tables()
                                        key=|table| (table.schema.clone(), table.name.clone())
                                        children=move |table| {
                                            view! { <TableCollapse vm=vm table=table show_schema=true /> }
                                        }
                                    />
                                </div>
                            }
                        }
                    >
                        <div class="flex flex-col gap-3">
                            <For
                                each=move || vm.visible_schemas()
                                key=|schema| schema.clone()
                                children=move |schema| view! { <SchemaGroup vm=vm schema=schema /> }
                            />
                        </div>
                    </Show>
                </Show>
            </Show>
        </div>
    }
}

/// Filter, expand/collapse all, bulk (un)ticking, the chosen count and Save/Discard — each of
/// those two asking for confirmation first.
#[component]
fn Toolbar(vm: ExplorerViewModel) -> impl IntoView {
    let save_dialog = NodeRef::<leptos::html::Dialog>::new();
    let discard_dialog = NodeRef::<leptos::html::Dialog>::new();

    view! {
        <ConfirmDialog
            node_ref=save_dialog
            title="Save ingested tables"
            message=move || save_message(vm)
            confirm_label="Save"
            on_confirm=move |_| vm.save()
        />
        <ConfirmDialog
            node_ref=discard_dialog
            title="Discard changes"
            message=move || {
                let (added, removed) = vm.pending_changes();
                format!(
                    "Discard your unsaved changes ({})? The tables go back to the last saved choice.",
                    change_summary(added, removed),
                )
            }
            confirm_label="Discard"
            danger=true
            on_confirm=move |_| vm.discard()
        />
        <div class="flex flex-col gap-3 lg:flex-row lg:items-center lg:justify-between">
            <div class="flex flex-wrap items-center gap-2">
                <TextInput
                    value=vm.filter
                    kind="search"
                    small=true
                    class="w-full sm:w-64"
                    placeholder="Filter tables"
                />
                <ExpandToggle
                    label="all"
                    all_open=Signal::derive(move || vm.all_visible_open())
                    disabled=Signal::derive(move || vm.visible_tables().is_empty())
                    on_toggle=move |_| vm.toggle_all_visible_open()
                />
                <div class="join">
                    <button
                        class="join-item btn btn-sm"
                        type="button"
                        on:click=move |_| vm.set_all_visible(true)
                    >
                        "Select shown"
                    </button>
                    <button
                        class="join-item btn btn-sm"
                        type="button"
                        on:click=move |_| vm.set_all_visible(false)
                    >
                        "Clear shown"
                    </button>
                </div>
            </div>
            <div class="flex flex-wrap items-center gap-2">
                <span class="text-sm text-base-content/70">
                    {move || {
                        format!(
                            "{} of {} tables chosen",
                            vm.chosen_count(),
                            vm.tables.with(Vec::len),
                        )
                    }}
                </span>
                <button
                    class="btn btn-sm"
                    type="button"
                    disabled=move || !vm.is_dirty() || vm.saving.get()
                    on:click=move |_| open_dialog(discard_dialog)
                >
                    "Discard"
                </button>
                <button
                    class="btn btn-sm btn-primary"
                    type="button"
                    disabled=move || !vm.is_dirty() || vm.saving.get()
                    on:click=move |_| open_dialog(save_dialog)
                >
                    "Save"
                </button>
            </div>
        </div>
    }
}

/// The save confirmation: what changes, and what that means for capture.
fn save_message(vm: ExplorerViewModel) -> String {
    let (added, removed) = vm.pending_changes();
    let mut message = format!(
        "Save the choice ({}), {} of {} tables in all?",
        change_summary(added, removed),
        vm.chosen_count(),
        vm.tables.with(Vec::len),
    );
    if added > 0 {
        message.push_str(
            " Newly chosen tables are captured from now on, without their earlier history.",
        );
    }
    if removed > 0 {
        message
            .push_str(" Unchosen tables stop being captured; what pipa already has of them stays.");
    }
    message
}

/// "2 added, 1 removed", leaving out a side with nothing.
fn change_summary(added: usize, removed: usize) -> String {
    match (added, removed) {
        (0, removed) => format!("{removed} removed"),
        (added, 0) => format!("{added} added"),
        (added, removed) => format!("{added} added, {removed} removed"),
    }
}

/// A Postgres schema as a collapse, parent of its tables' collapses (those the filter shows).
/// Its title folds them away or shows them again, without touching which tables are expanded;
/// its checkbox ticks or unticks them all — indeterminate while only some are ticked. Open
/// while it is in `vm.open_schemas` (it starts collapsed).
#[component]
fn SchemaGroup(vm: ExplorerViewModel, schema: String) -> impl IntoView {
    let choice = Memo::new({
        let schema = schema.clone();
        move |_| vm.schema_choice(&schema)
    });
    let is_open = Memo::new({
        let schema = schema.clone();
        move |_| vm.is_schema_open(&schema)
    });
    let schema = StoredValue::new(schema);

    view! {
        <Collapse
            open=is_open
            on_toggle=move |_| vm.toggle_schema(&schema.read_value())
            background="bg-base-200"
            title=move || {
                view! {
                    <div class="flex flex-wrap items-center gap-x-3">
                        <Checkbox
                            primary=true
                            small=true
                            stop_click=true
                            label=format!("Ingest every table of {}", schema.read_value())
                            checked=Signal::derive(move || {
                                let (chosen, shown) = choice.get();
                                shown > 0 && chosen == shown
                            })
                            indeterminate=Signal::derive(move || {
                                let (chosen, shown) = choice.get();
                                chosen > 0 && chosen < shown
                            })
                            on_change=move |on| vm.set_schema_chosen(&schema.read_value(), on)
                        />
                        <h2 class="font-mono text-lg font-semibold break-all">
                            {schema.get_value()}
                        </h2>
                        <span class="text-sm text-base-content/70">
                            {move || {
                                let (chosen, shown) = choice.get();
                                format!("{chosen} of {shown} chosen")
                            }}
                        </span>
                    </div>
                }
            }
        >
            <div class="flex flex-col gap-2">
                <For
                    each=move || vm.visible_tables_in(&schema.read_value())
                    key=|table| table.name.clone()
                    children=move |table| {
                        view! { <TableCollapse vm=vm table=table show_schema=false /> }
                    }
                />
            </div>
        </Collapse>
    }
}

/// One table: a checkbox choosing it, its name and column count as the title, its columns
/// inside. Expanded while it is in `vm.open`; any number can be open at once.
#[component]
fn TableCollapse(
    vm: ExplorerViewModel,
    table: SourceTableView,
    /// Prefix the name with its schema; off under a [`SchemaGroup`] heading, which already
    /// says it.
    show_schema: bool,
) -> impl IntoView {
    let reference = StoredValue::new(table_ref(&table));
    let is_open = Memo::new(move |_| vm.is_open(&reference.read_value()));
    let column_count = match table.columns.len() {
        1 => "1 column".to_string(),
        count => format!("{count} columns"),
    };
    let columns = StoredValue::new(table.columns);

    view! {
        <Collapse
            open=is_open
            on_toggle=move |_| vm.toggle_open(reference.get_value())
            title=move || {
                let table = reference.get_value();
                view! {
                    <div class="flex items-center gap-3">
                        <Checkbox
                            primary=true
                            small=true
                            stop_click=true
                            label=format!("Ingest {}.{}", table.schema, table.name)
                            checked=Signal::derive(move || vm.is_chosen(&reference.read_value()))
                            on_change=move |on| vm.set_chosen(reference.get_value(), on)
                        />
                        <div class="flex min-w-0 flex-wrap items-baseline gap-x-3">
                            <span class="font-mono font-semibold break-all">
                                {show_schema
                                    .then(|| {
                                        view! {
                                            <span class="text-base-content/60">
                                                {format!("{}.", table.schema)}
                                            </span>
                                        }
                                    })}
                                {table.name.clone()}
                            </span>
                            <span class="text-sm text-base-content/70">{column_count.clone()}</span>
                            <Show when=move || vm.is_ingested(&reference.read_value())>
                                <Badge tone=Tone::Success style=BadgeStyle::Soft small=true>
                                    "ingested"
                                </Badge>
                            </Show>
                        </div>
                    </div>
                }
            }
        >
            <Columns columns=columns.get_value() />
        </Collapse>
    }
}

/// A table's columns: a table on desktop, cards on mobile.
#[component]
fn Columns(columns: Vec<SourceColumnView>) -> impl IntoView {
    let for_table = columns.clone();
    let for_cards = columns;

    view! {
        <ResponsiveList
            table=move || {
                let columns = for_table.clone();
                view! {
                    <div class="overflow-x-auto">
                        <table class="table table-sm">
                            <thead>
                                <tr>
                                    <th>"Column"</th>
                                    <th>"Type"</th>
                                    <th>"Nullable"</th>
                                    <th>"Key"</th>
                                    <th>"References"</th>
                                </tr>
                            </thead>
                            <tbody>
                                {columns
                                    .into_iter()
                                    .map(|column| {
                                        let references = column.foreign_key.as_ref().map(reference_label);
                                        view! {
                                            <tr>
                                                <td class="font-mono">{column.name}</td>
                                                <td class="font-mono text-base-content/70">{column.data_type}</td>
                                                <td>{if column.nullable { "yes" } else { "no" }}</td>
                                                <td>{column.primary_key.then(|| view! { <PrimaryBadge /> })}</td>
                                                <td class="font-mono text-base-content/70">{references}</td>
                                            </tr>
                                        }
                                    })
                                    .collect_view()}
                            </tbody>
                        </table>
                    </div>
                }
            }
            cards=move || {
                for_cards
                    .clone()
                    .into_iter()
                    .map(|column| {
                        let name = column.name;
                        let primary = column.primary_key;
                        let kind = format!(
                            "{}{}",
                            column.data_type,
                            if column.nullable { ", nullable" } else { ", not null" },
                        );
                        let references = column
                            .foreign_key
                            .as_ref()
                            .map(|reference| format!("\u{2192} {}", reference_label(reference)));
                        view! {
                            <Card compact=true body_class="gap-1">
                                <div class="flex items-start justify-between gap-2">
                                    <span class="font-mono font-semibold break-all">{name}</span>
                                    {primary.then(|| view! { <PrimaryBadge /> })}
                                </div>
                                <span class="font-mono text-sm break-all text-base-content/70">
                                    {kind}
                                </span>
                                {references
                                    .map(|references| {
                                        view! {
                                            <span class="font-mono text-sm break-all text-base-content/70">
                                                {references}
                                            </span>
                                        }
                                    })}
                            </Card>
                        }
                    })
                    .collect_view()
            }
        />
    }
}

/// Marks a primary key column.
#[component]
fn PrimaryBadge() -> impl IntoView {
    view! {
        <Badge style=BadgeStyle::Outline small=true class="shrink-0">
            "primary"
        </Badge>
    }
}

/// A foreign key's target as `schema.table.column`.
fn reference_label(reference: &SourceColumnRef) -> String {
    format!(
        "{}.{}.{}",
        reference.schema, reference.table, reference.column
    )
}
