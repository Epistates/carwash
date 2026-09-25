# carwash-core

The engine behind [carwash](https://github.com/epistates/carwash), with no UI code.

It finds projects of 40 ecosystems under a directory, recognises their build outputs,
dependency installs, caches and environments, measures what deleting them would really free
(allocated blocks, hard links counted once), asks git what is tracked or ignored, and deletes
safely. It also discovers project tasks, checks dependencies against crates.io, npm, PyPI and
the Go proxy (with OSV advisories), and lists per-user caches outside projects.

Most users want the [`carwash`](https://crates.io/crates/carwash) command-line tool instead.
The API follows carwash's needs and may change between minor versions.

License: MIT
