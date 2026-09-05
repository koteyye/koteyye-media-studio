import fs from 'fs';
import path from 'path';
import { validateImage } from './image-validator';
import type { UploadedFrameImage } from '../../shared/ipc-types';

const MAX_AUDIO_SIZE = 30 * 1024 * 1024;
const ALLOWED_AUDIO_EXTENSIONS = ['.mp3', '.wav', '.aac', '.m4a', '.ogg', '.flac'];

const AUDIO_MIME_MAP: Record<string, string> = {
  '.mp3': 'audio/mpeg',
  '.wav': 'audio/wav',
  '.aac': 'audio/aac',
  '.m4a': 'audio/x-m4a',
  '.ogg': 'audio/ogg',
  '.flac': 'audio/flac',
};

export async function loadFrameImage(filePath: string): Promise<UploadedFrameImage> {
  const validation = validateImage(filePath);
  if (!validation.valid) throw new Error(validation.error);

  const buffer = fs.readFileSync(filePath);
  const url = `data:${validation.mimeType};base64,${buffer.toString('base64')}`;

  return {
    frameType: 'first_frame',
    url,
    originalFileName: validation.fileName,
    mimeType: validation.mimeType,
    sizeBytes: validation.sizeBytes,
    readAt: new Date().toISOString(),
  };
}

export async function loadAudioReference(filePath: string): Promise<UploadedFrameImage> {
  const fileName = path.basename(filePath);
  if (!fs.existsSync(filePath)) {
    throw new Error('File does not exist');
  }
  const stat = fs.statSync(filePath);
  if (stat.size > MAX_AUDIO_SIZE) {
    throw new Error(`File too large (${(stat.size / 1024 / 1024).toFixed(1)} MB). Maximum: 30 MB`);
  }
  const ext = path.extname(fileName).toLowerCase();
  if (!ALLOWED_AUDIO_EXTENSIONS.includes(ext)) {
    throw new Error(`Unsupported audio format: ${ext}`);
  }

  const buffer = fs.readFileSync(filePath);
  const mimeType = AUDIO_MIME_MAP[ext] || 'audio/mpeg';
  const url = `data:${mimeType};base64,${buffer.toString('base64')}`;

  return {
    frameType: 'first_frame',
    url,
    originalFileName: fileName,
    mimeType,
    sizeBytes: stat.size,
    readAt: new Date().toISOString(),
  };
}
