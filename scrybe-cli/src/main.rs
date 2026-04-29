// Copyright 2026 Mathews Tom
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     https://www.apache.org/licenses/LICENSE-2.0

fn main() {
    println!(
        "{name} v{version} — placeholder; see {repository}",
        name = scrybe::NAME,
        version = scrybe::VERSION,
        repository = scrybe::REPOSITORY,
    );
}
