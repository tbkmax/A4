# Local Agent demo

A Tauri 2 desktop app with an Alpine.js interface. It checks local Ollama, lets you choose a workspace and model in Settings, turns model responses into actions, runs them, and keeps a local audit log.

## Run

1. Install the [Tauri 2 prerequisites](https://v2.tauri.app/start/prerequisites/) for your OS. On Windows, include the Microsoft C++ Build Tools **and Windows SDK**. If the SDK is only in `On_top_Clock/.tools`, the included launcher uses that copy automatically. You can point `LOCAL_AGENT_SDK_TOOLS` to another folder containing `sdk-core` and `sdk-x64`.
2. Install [Ollama](https://ollama.com/download), start it, and download a model, for example `ollama pull llama3.2`.
3. Run `npm install` and `npm run tauri dev` from this folder. On Windows, the `tauri` script loads MSVC and the SDK before starting Tauri. Run `npm run check:native` for a Rust compile check or `npm run test:native` for the path validation tests.

The app talks to Ollama at `http://127.0.0.1:11434`. No API key is needed. A model must appear in Ollama's `/api/tags` response before the chat opens.

Use **New conversation** to clear chat history without changing the working folder or model.

## GitHub Actions builds

The [Desktop builds workflow](.github/workflows/desktop-build.yml) runs on pushes, pull requests, and manual dispatch. It tests the app and packages macOS Apple Silicon, macOS Intel, and Windows x64 builds. Download the installers from the workflow run's **Artifacts** section. The macOS builds use ad-hoc signing, so macOS may ask you to allow the app in Privacy & Security when opening it.
