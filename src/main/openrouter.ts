import type {
  OpenRouterModel,
  OpenRouterCapabilityDescriptor,
  OpenRouterInputReference,
  VideoGenerateParams,
  VideoJobResult,
  VideoJobStatus,
  ImageGenerationResult,
  ImageGenerateOptions,
} from '../shared/ipc-types';

const BASE_URL = 'https://openrouter.ai/api/v1';

function enumValues(descriptor: OpenRouterCapabilityDescriptor | undefined): string[] | undefined {
  if (descriptor && descriptor.type === 'enum' && Array.isArray(descriptor.values)) {
    return descriptor.values;
  }
  return undefined;
}

export async function fetchImageModels(apiKey: string): Promise<OpenRouterModel[]> {
  const headers = { Authorization: `Bearer ${apiKey}` };

  const [imagesRes, legacyRes] = await Promise.allSettled([
    fetch(`${BASE_URL}/images/models`, { headers }),
    fetch(`${BASE_URL}/models?output_modalities=image`, { headers }),
  ]);

  const legacyModels = legacyRes.status === 'fulfilled' && legacyRes.value.ok
    ? (((await legacyRes.value.json()) as { data?: OpenRouterModel[] }).data ?? [])
    : null;

  if (imagesRes.status === 'fulfilled' && imagesRes.value.ok) {
    const json = (await imagesRes.value.json()) as { data?: OpenRouterModel[] };
    const base = json.data ?? [];
    if (base.length === 0 && legacyModels) return legacyModels;
    const legacyById = new Map((legacyModels ?? []).map((m) => [m.id, m]));

    return base.map((m) => {
      const legacy = legacyById.get(m.id);
      const params = m.supported_parameters ?? {};
      return {
        ...m,
        architecture: m.architecture ?? legacy?.architecture,
        pricing: legacy?.pricing,
        supported_resolutions: enumValues(params.resolution) ?? m.supported_resolutions,
        supported_aspect_ratios: enumValues(params.aspect_ratio) ?? m.supported_aspect_ratios,
        supported_sizes: enumValues(params.size) ?? m.supported_sizes,
      } as OpenRouterModel;
    });
  }

  if (legacyModels) return legacyModels;

  const reason = imagesRes.status === 'rejected'
    ? String(imagesRes.reason)
    : `Failed to fetch image models: ${imagesRes.value.status}`;
  throw new Error(reason);
}

export async function fetchVideoModels(apiKey: string): Promise<OpenRouterModel[]> {
  const res = await fetch(`${BASE_URL}/videos/models`, {
    headers: { Authorization: `Bearer ${apiKey}` },
  });
  if (!res.ok) {
    const body = await res.text();
    throw new Error(`Failed to fetch video models: ${res.status} - ${body}`);
  }
  const json = await res.json();
  return (json.data ?? []) as OpenRouterModel[];
}

export async function generateVideo(
  apiKey: string,
  model: string,
  params: VideoGenerateParams
): Promise<VideoJobResult> {
  const body: Record<string, unknown> = { model, prompt: params.prompt };
  if (params.resolution) body.resolution = params.resolution;
  if (params.aspect_ratio) body.aspect_ratio = params.aspect_ratio;
  if (params.size) body.size = params.size;
  if (params.duration !== undefined && params.duration > 0) body.duration = params.duration;
  if (params.seed !== undefined) body.seed = params.seed;
  if (params.generate_audio !== undefined) {
    body.generate_audio = params.generate_audio;
  }
  if (params.frame_images) body.frame_images = params.frame_images;
  if (params.input_references) body.input_references = params.input_references;

  const res = await fetch(`${BASE_URL}/videos`, {
    method: 'POST',
    headers: {
      Authorization: `Bearer ${apiKey}`,
      'Content-Type': 'application/json',
    },
    body: JSON.stringify(body),
  });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`Video generation request failed: ${res.status} - ${text}`);
  }
  const json = await res.json();
  return { id: json.id, polling_url: json.polling_url };
}

export async function generateImage(
  apiKey: string,
  model: string,
  prompt: string,
  references?: OpenRouterInputReference[],
  options?: ImageGenerateOptions
): Promise<ImageGenerationResult> {
  const body: Record<string, unknown> = { model, prompt };
  if (references && references.length > 0) body.input_references = references;
  if (options?.resolution) body.resolution = options.resolution;
  if (options?.aspectRatio) body.aspect_ratio = options.aspectRatio;
  if (options?.seed !== undefined) body.seed = options.seed;

  const res = await fetch(`${BASE_URL}/images`, {
    method: 'POST',
    headers: {
      Authorization: `Bearer ${apiKey}`,
      'Content-Type': 'application/json',
    },
    body: JSON.stringify(body),
  });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`Image generation request failed: ${res.status} - ${text}`);
  }
  const json = await res.json();

  const images = Array.isArray(json.data)
    ? json.data
        .filter((item: unknown): item is { b64_json: string; media_type?: string } =>
          !!item && typeof (item as { b64_json?: unknown }).b64_json === 'string')
        .map((item: { b64_json: string; media_type?: string }) => ({
          b64_json: item.b64_json,
          media_type: item.media_type,
        }))
    : [];

  return {
    images,
    model: json.model ?? model,
    usage: json.usage,
  };
}

export async function fetchCredits(apiKey: string): Promise<{ total_credits: number; total_usage: number }> {
  const res = await fetch(`${BASE_URL}/credits`, {
    headers: { Authorization: `Bearer ${apiKey}` },
  });
  if (!res.ok) {
    const body = await res.text();
    throw new Error(`Failed to fetch credits: ${res.status} - ${body}`);
  }
  const json = await res.json();
  const data = json.data ?? {};
  return {
    total_credits: data.total_credits ?? 0,
    total_usage: data.total_usage ?? 0,
  };
}

export async function pollVideoStatus(
  apiKey: string,
  pollingUrl: string
): Promise<VideoJobStatus> {
  const res = await fetch(pollingUrl, {
    headers: { Authorization: `Bearer ${apiKey}` },
  });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`Polling failed: ${res.status} - ${text}`);
  }
  return (await res.json()) as VideoJobStatus;
}

export async function downloadVideoContent(
  apiKey: string,
  jobId: string,
  outputPath: string
): Promise<string> {
  const res = await fetch(`${BASE_URL}/videos/${jobId}/content`, {
    headers: { Authorization: `Bearer ${apiKey}` },
  });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(`Download failed: ${res.status} - ${text}`);
  }
  const buffer = await res.arrayBuffer();
  const fs = await import('fs');
  fs.writeFileSync(outputPath, Buffer.from(buffer));
  return outputPath;
}
