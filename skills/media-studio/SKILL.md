---
name: media-studio
description: Generate images, video, speech, music, or sound effects in Koteyye Media Studio through its local MCP server. Use for the studio's model catalog and to retrieve locally saved results from its generation history.
---

# Koteyye Media Studio

- Call `studio_status` to check the active provider, generation readiness, and `model_defaults` for that provider.
- If the requested mode has a fixed model in `model_defaults`, omit `request.model` so the studio uses the user's selection. Use `list_models` to inspect that model's limits when needed. An absent mode means Auto. In Auto, choose a suitable model from `list_models` and pass its exact ID in `request.model`.
- Call `start_generation` only when the user has asked for an actual generation. The active provider can charge for the request. Pass `provider` from `studio_status`. Supply a fresh UUID in `idempotency_key` for each distinct generation. If the connection drops during submission, reuse the same key and arguments while the job remains in history.
- The start result is a history entry, typically with status `pending`. Poll `get_generation` with its `id`, allowing time between calls. On `completed`, give the user the local file paths and the cost if reported. On `failed`, report the diagnostic and request ID when present. If a job is still pending when you stop waiting, return its history ID so it can be checked later.

Each `request` includes `kind`, `mode` and `prompt`. For Cheaper Inference video, include `params.duration_seconds` and use the model's supported durations. Send references as HTTP(S) URLs or supported image data URLs in `request.references`. For images, use `text_to_image` without references and `image_edit` with references. For `image_to_video`, the single reference is the first frame. For `reference_to_video`, choose a model that accepts reference images. An image-edit mask is a CI-only PNG data URL.

The desktop app owns the provider credentials and the output folder. Use the local MCP bearer token only for this connection. A connection check can list tools, read this skill, or call `studio_status` without starting a generation.
