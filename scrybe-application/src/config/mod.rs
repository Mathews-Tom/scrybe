// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Configuration contracts.
//!
//! The read projection ([`ConfigForm`]) and the write surface
//! ([`ConfigUpdate`]) are deliberately different shapes. The form
//! carries everything a settings screen renders; the update surface is
//! a closed set of [`ConfigField`]s a GUI owns. Fields that hold or
//! point at credentials appear in neither, so writing a secret through
//! this layer is unrepresentable rather than merely disallowed.

mod contract;

pub use contract::{
    ConfigDiagnostic, ConfigField, ConfigForm, ConfigSnapshot, ConfigUpdate, ConfigValue,
    ConfigValueKind, EDITABLE_FIELDS,
};
