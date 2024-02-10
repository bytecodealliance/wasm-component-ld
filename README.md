# `wasm-component-ld`

A work-in-progress and/or proof-of-concept linker which wraps `wasm-ld` and then
executes `wit-component` to produce a component output instead of a core wasm
output.

# License

This project is triple licenced under the Apache 2/ Apache 2 with LLVM exceptions/ MIT licences. The reasoning for this is:
- Apache 2/ MIT is common in the rust ecosystem.
- Apache 2/ MIT is used in the rust standard library, and some of this code may be migrated there.
- Some of this code may be used in compiler output, and the Apache 2 with LLVM exceptions licence is useful for this.

For more details see
- [Apache 2 Licence](LICENSE-APACHE)
- [Apache 2 Licence with LLVM exceptions](LICENSE-Apache-2.0_WITH_LLVM-exception)
- [MIT Licence](LICENSE-MIT)

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this project by you, as defined in the Apache 2/ Apache 2 with LLVM exceptions/ MIT licenses,
shall be licensed as above, without any additional terms or conditions.
