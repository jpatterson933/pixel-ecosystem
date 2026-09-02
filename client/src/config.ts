function stripSlash(url: string): string {
  return url.replace(/\/+$/, '')
}

export const GATEWAY_URL = stripSlash(import.meta.env.VITE_GATEWAY_URL ?? 'http://localhost:8080')
export const TILE_BASE_URL = stripSlash(import.meta.env.VITE_TILE_BASE_URL ?? 'http://localhost:8082')
export const WS_URL = GATEWAY_URL.replace(/^http/, 'ws') + '/ws'

export const CHUNK_SIZE = 256
export const TILE_SIZE = 256
export const MAX_ZOOM = 64

export const SUBSCRIBE_DEBOUNCE_MS = 100
export const STATUS_INTERVAL_MS = 250
