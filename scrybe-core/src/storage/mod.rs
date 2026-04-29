// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

//! Storage primitives: platform-correct atomic replace and durable
//! append. Public surface kept narrow because every caller is the
//! pipeline or `scrybe doctor`; adapter crates do not write directly.

mod atomic;

pub use atomic::{append_durable, atomic_replace, full_fsync};
