# Souvenirs 2

A reboot of [Souvenirs](https://github.com/yoannchaudet/souvenirs) for organizing long-term photo and video collections as ordinary files.

This repository currently contains a proposed specification, not an implementation.
Start with the [specification guide](docs/README.md).

## Run deterministic engine scenarios

Build the fake engine for macOS interface development:

```sh
cargo build -p souvenirs-engine --bin souvenirs-fake-engine
```

In the Xcode scheme's **Run > Arguments > Environment Variables** settings, set:

```text
SOUVENIRS_ENGINE_EXECUTABLE=/absolute/path/to/target/debug/souvenirs-fake-engine
SOUVENIRS_FAKE_ENGINE_SCENARIO=successful-operation
```

Available scenarios are `ready`, `successful-operation`, `failed-operation`,
`cancellation-race`, `unexpected-termination`, and `malformed-response`. These
scenarios use standard input and output only. The app currently performs only
the handshake, so it shows the `ready` state for operation scenarios until an
interface calls the `EngineSupervisor.requestOperation` API. The production
engine remains the default when either environment variable is absent.
