# TLA+ Tools

This directory holds optional tooling that is not committed to the repository.
Files listed in `.gitignore` must be obtained locally.

## tla2tools.jar

Download `tla2tools.jar` to enable the `run_tlc` validator in
`spec validate --lane full`. Without it the validator is skipped with a warning.

```bash
curl -L -o tools/tla2tools.jar \
  https://github.com/tlaplus/tlaplus/releases/latest/download/tla2tools.jar
```

The `TLA2TOOLS_JAR` environment variable is set automatically by `mise` (see
`mise.toml`) to point at this path. If you use a different workflow, export it
manually:

```bash
export TLA2TOOLS_JAR="$(pwd)/tools/tla2tools.jar"
```

### Verify the download

```bash
java -jar tools/tla2tools.jar -version
```

Expected output starts with `TLC version ...`.

### Run TLC manually

```bash
java -jar tools/tla2tools.jar \
  -config docs/arch/JobRegistry.cfg \
  docs/arch/JobRegistry.tla
```

Or use the spec framework wrapper which sets up the classpath and output format:

```bash
spec validate --lane full
```
