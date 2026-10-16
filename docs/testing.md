# Testing

`cooper test <project-dir> [name-filter]` builds and runs a project's tests.

## Writing tests

A test is a function named `test` followed by a capital letter or digit, taking nothing and
returning nothing, in a file whose name ends in `_test.coop`. The file belongs to the module of
its directory, so a test sees everything the module declares. An ordinary build (`build`, `run`)
leaves `*_test.coop` files out; `check` reads them.

```
# mathx/mathx_test.coop
use {
  std.testing.check,
}

func testAdd() {
  check(add(2, 3) == 5, "2 + 3")
}
```

`std.testing` has `check(ok: bool, message: string)`, which fails the test with `message` unless
`ok` holds, and `fail(message)`. A test passes when it returns. It fails when it calls `fail`,
when a `check` does not hold, or when the program panics. Any other function in a test file is a
helper, not a test.

## Running them

All the tests are compiled into one program, which runs the one test its `COOPER_TEST`
environment variable selects, and each test runs in a process of its own. A failure or a panic
therefore ends that test and no other, and tests cannot affect one another's state. A report line
names each test as `module.testName`, a failing test shows what it wrote, and the exit status is
nonzero when any test failed.

```
ok    mathx.testAdd
FAIL  mathx.testSecret
        FAIL: secret is 41

1 passed, 1 failed
```

The optional name filter runs the tests whose name contains it. A project may be a library: tests
need no `main`.

## Not yet

Failure messages do not say where in the test they came from, tests run one after another, and
there is no timeout, set-up and tear-down, or benchmark support.
