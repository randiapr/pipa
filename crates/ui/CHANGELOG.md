# Changelog

All notable changes to `pipa-ui` are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this crate adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
(pre-1.0: MINOR bumps may include breaking changes).

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
