# tentris-license-aggregator

A library based on [cargo-about](https://github.com/EmbarkStudios/cargo-about) to collect
rust licenses and to augment licenses of non-rust dependencies scraped by a third party tool that may
not have knowledge about the SPDX identifiers of individual license files (e.g. conan).

The output format is different to cargo-about, tentris-license-aggregator does not accumulate
all dependencies that have a specific license, instead it accumulates all licenses for each dependency.
