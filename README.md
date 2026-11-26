<p align="center">
  <img src="ui/assets/icons/koteyye_media_studio_logo.png" width="128" alt="Koteyye Media Studio logo" />
</p>

# Koteyye Media Studio v2

[Русская версия](readme_ru.md)

Koteyye Media Studio is a desktop app for working with AI media models from
**Cheaper Inference** and **OpenRouter**. Generate images, video, speech, music,
and sound effects in one app and save the results locally. You can connect both
providers and choose which one to use in the studio.

Add your API keys in **Settings → Connection**. Available models depend on the
provider.

## Connect an agent via MCP

Enable the local server in **Settings → MCP**, then copy the connection prompt
into your agent development environment (ADE). The agent can browse models,
generate media, and retrieve the generated files through the studio.

The server runs at `http://127.0.0.1:29731/mcp`. The connection prompt includes
instructions for Streamable HTTP and stdio clients. Keep the MCP token private
because it grants access to generation, including paid requests.

## Run from source

Install Rust stable with the `wasm32-unknown-unknown` target, Tauri CLI 2, and
Dioxus CLI 0.7.10. Run `cargo tauri dev` from the repository root.
