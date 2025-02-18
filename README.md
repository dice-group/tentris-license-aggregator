# tentris-license-aggregator

A library based on [cargo-about](https://github.com/EmbarkStudios/cargo-about) to collect
rust licenses and to augment licenses of non-rust dependencies scraped by a third party tool that may
not have knowledge about the SPDX identifiers of individual license files (e.g. conan).

### Differences to cargo-about
1. While `cargo-about` accumulates all dependencies that have a specific license,
   `tentris-license-aggregator` accumulates all licenses for each dependency
   (i.e. the collected results are `Vec<tentris_license_aggregator::Package>` (see `pub struct Package` in [lib.rs](src/lib.rs)).
2. `tentris-license-aggregator` allows to detect SPDX identifiers for licenses collected by a third party tool (e.g. conan)
    that does not know them
