# Changelog

All notable changes to `pipa-ui` are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this crate adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
(pre-1.0: MINOR bumps may include breaking changes).

## [0.4.1] - 2026-10-07

### Changed

- The `/dashboard` route is gone: its Projects and Sources cards now live on their own
  routes, `/projects` and `/sources` (previously `/dashboard#projects` and
  `/dashboard#sources`). Nav links and the landing page's links point at them.
- The **Register a data source** form no longer has a project picker; the data source is
  registered into the project selected in the nav-bar switcher (none for an admin on
  "All projects").

### Dependencies

- Bumped `leptos` to 0.8.22 and `leptos_router` to 0.8.17.

## [0.4.0] - 2026-09-30

### Added

- **Sign-in** page and session handling: the token is kept in `localStorage`, sent on every
  request, and dropped when the backend answers 401. Routes redirect to `/login` when signed
  out.
- **Roles.** The **Users** page (create, edit role/password/projects, delete) and its nav
  entry are admin-only; `user` accounts don't see the menu and are redirected away from
  `/users`. Creating, editing and deleting projects is likewise admin-only.
- **Project context.** A project switcher in the nav bar (desktop and drawer) scopes the
  dashboard's data sources, the register form's default project, and the new **Query** page
  to the selected project. Admins also get "All projects". The selection persists across
  reloads. The landing page's "Manage" button selects that project.
- **Query** page: read-only SQL over the selected project's tables.

### Changed

- The container image (`Dockerfile`'s `ui` stage) is now served by static-web-server instead of
  nginx, with an SPA fallback to `index.html`, so reloading or deep-linking a client route
  (`/login`, `/users`, `/query`, ...) no longer 404s.

## [0.3.2] - 2026-09-29

### Changed

- **`src/api.rs` and the viewmodels/views now use the `pipa-api` contract crate** for every
  request/response type and route path; `src/model.rs` (a hand-maintained mirror of the
  backend's wire types) is removed. `src/api.rs` also shares `send`/`send_discard` helpers
  across endpoints. No user-visible behavior change.
- `DbEngine::Mysql` is now `DbEngine::MySql`, matching the contract crate.

## [0.3.1] - 2026-09-28

### Changed

- `src/api.rs` updated to match `pipa-backend` 0.5.2's new response envelope: list/single
  project and data source responses are now unwrapped from their `projects`/`project`/
  `datasources`/`datasource`/`connection_test` envelope fields instead of being read as bare
  JSON. No user-visible behavior change — purely an internal wire-format adjustment.

## [0.3.0] - 2026-09-24

### Added

- Delete confirmation dialogs for projects and data sources, naming the item being removed,
  instead of deleting on a single click.
- Explicit close (`✕`) buttons on the create/edit/register dialogs.
- A light/dark theme toggle in the navbar and mobile drawer, in the spot the "Home" nav
  entry used to occupy — the brand link (`pipa`) now covers going back to `/`.
- `StatusMessage` (`Success`/`Error`) picks the matching daisyUI `alert` variant for a
  dashboard status message, shown as an auto-dismissing toast (`toast-top toast-end`)
  instead of an inline banner.
- `SourcesViewModel::name_of`, so the delete-source confirmation dialog can show the
  source's name from just its id.

### Changed

- Edit/delete row actions in the projects and sources tables now render as icon buttons
  instead of text buttons.
- Cards use `card-border`/`shadow-xl` instead of `shadow-sm` for a more defined boundary.
- Registering or renaming a project now surfaces the backend's duplicate-name rejection
  (see `pipa-data` 0.2.1) as a status toast instead of silently failing.

## [0.2.0] - 2026-09-23

### Added

- **Projects**: a new card on the dashboard (`/dashboard#projects`) to create, rename/
  re-describe, and delete projects, and a project picker on the data source registration
  form to group sources under one. A new landing page (`/`) shows every project as a card
  (name, description, data source count) with a link into the dashboard.
- Create-project, edit-project, and register-data-source forms now open as native
  `<dialog class="modal">` dialogs (`showModal()`/`close()`) instead of inline forms,
  dismissible via their Cancel button, a backdrop click, or Esc, with form state reset on
  every close regardless of cause.
- Registered data sources and projects are now shown in daisyUI `table`s inside `card`s,
  paginated client-side (5 rows/page) with a shared `Pagination` component.

### Changed

- Restructured the crate as MVVM: `model` (wire/domain types, split out of `api`), `api`
  (pure HTTP client), `viewmodel` (`ProjectsViewModel`/`SourcesViewModel`/`AppViewModel` —
  `RwSignal`-backed state and the only code that calls `api`), and `view` (render-only
  Leptos components that call ViewModel methods).
- The dashboard moved from `/` to `/dashboard`; `/` is now the projects landing page. Nav
  updated to Home / Projects / Sources (the separate "Register" nav entry was folded into
  the Sources card's new-data-source dialog).

### Fixed

- A `disabled=move || ... >= ...` attribute closure in `Pagination` tripped a Leptos
  `view!` macro parsing gotcha — a bare (non-`{}`-wrapped) top-level `>=`/`>` in an
  attribute expression is misparsed as tag-closing syntax, dumping raw token text into the
  DOM as visible button text. Fixed by wrapping the closure in braces.
- The register-data-source dialog's `.modal-box` could exceed the viewport height on
  shorter screens with no way to reach the submit button; it now caps at 85vh and scrolls
  internally (applied to all three dialogs for consistency).

## [0.1.0] - 2026-09-22

### Added

- Leptos (CSR) dashboard for registering and managing OLTP data sources: a registration
  form (name/engine/host/port/username/password/database) and a list of registered
  sources with test-connection and remove actions, talking to `pipa-rest` over HTTP
  (`src/api.rs`).
- Tailwind CSS v4 + daisyUI styling, built entirely through Trunk's own Tailwind asset
  pipeline (`Trunk.toml`'s `[tools] tailwindcss`, `tailwind.css`) — no separate npm/vite
  build step; `npm install` is only needed to resolve daisyUI as a Tailwind plugin.
- Responsive navigation shell: a desktop navbar with a horizontal menu, and an off-canvas
  sidebar drawer on mobile (daisyUI's drawer component), with nav-link clicks closing the
  drawer.
