// Copyright (c) 2026 Randi Apriansyah
// SPDX-License-Identifier: MIT

//! Infrastructure layer: concrete adapters for the project domain's ports.

mod repository;

pub use repository::ObjectStoreProjectRepository;
