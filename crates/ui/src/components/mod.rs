//! Generic daisyUI components: the building blocks every page is made of.
//!
//! Nothing here knows about pipa — no `crate::viewmodel`, `crate::api` or `pipa_api` imports.
//! A component takes plain values, signals and callbacks, and renders one daisyUI component
//! (`modal`, `collapse`, `card`, `alert`, `toast`, `badge`, `loading`, `fieldset`/`input`/
//! `textarea`/`select`/`checkbox`) with the classes pipa uses for it, so every page looks and
//! behaves the same. Pages (`crate::view`) compose these with their ViewModels.
//!
//! Variants are enums mapped to complete class names (`"alert-success"`, never
//! `format!("alert-{tone}")`): Tailwind only generates the classes it finds spelled out in
//! the `.rs` sources (`@source` in `tailwind.css`).
//!
//! Plain buttons and `join` groups stay as markup (`class="btn btn-sm"`): they carry their own
//! click handlers and labels, and a wrapper would only rename the classes. The app shell
//! (navbar, drawer, theme switch) is used once, so it lives with `App` in `crate::view`.

mod card;
mod collapse;
mod feedback;
mod form;
mod icons;
mod modal;

pub use card::{Card, CardActions, CardTitle};
pub use collapse::{Collapse, ExpandToggle};
pub use feedback::{Alert, Badge, BadgeStyle, Loading, Toast, Tone};
pub use form::{Checkbox, Field, Select, TextArea, TextInput};
pub use icons::{CollapseAllIcon, EditIcon, ExpandAllIcon, TrashIcon};
pub use modal::{ConfirmDialog, Modal, ModalActions, close_dialog, open_dialog};
