/*
 * The Cooper runtime, linked into every program.
 *
 * It owns the process entry point: the C `main` runs any runtime setup and then the
 * program's own `main`, which the compiler exports as `cooper_main` returning the
 * process exit status.
 */

extern int cooper_main(void);

int main(void) {
    return cooper_main();
}
