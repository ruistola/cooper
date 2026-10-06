# Working title: Cooper

A compiler, written in Rust, for a compiled, statically typed, general purpose programming language.
This is primarily a project for learning programming language design and to study how compilers are implemented, but
the compiler is still being built according to known best practices and industry standards.

Disclaimer: This is a side project that may or may not see updates.

## Usage

```
cargo run -- run examples/hello.coop     # build and run a file
cargo run -- check examples/demo         # check a project
cargo run -- build examples/demo         # build an executable into examples/demo/build/
```

Building needs `clang` on the `PATH` (or a compatible compiler named by `COOPER_CC`).
