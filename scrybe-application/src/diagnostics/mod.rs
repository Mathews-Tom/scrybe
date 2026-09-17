// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Diagnostics contracts.
//!
//! Diagnosis and repair are two separate surfaces by construction. A
//! [`DiagnosticFinding`] is inert data: it describes what is wrong and,
//! when something can be done about it, names the [`RecoveryAction`] a
//! user would have to choose. Nothing in this module performs a
//! mutation, so opening a diagnostics screen cannot change the system.

mod contract;

pub use contract::{
    DiagnosticCode, DiagnosticComponent, DiagnosticFinding, DiagnosticReport, RecoveryAction,
    RepairApplication, RepairStatus, Severity,
};
