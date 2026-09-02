export interface GraphicsSettings {
  liveChunkLimit: number
  tileRefreshMs: number
  densityGain: number
  frameRateCap: number
  prediction: boolean
}

export const STORAGE_KEY = 'pe.graphics'

export const DEFAULT_SETTINGS: GraphicsSettings = {
  liveChunkLimit: 64,
  tileRefreshMs: 1000,
  densityGain: 1.0,
  frameRateCap: 60,
  prediction: true,
}

export const BOUNDS = {
  liveChunkLimit: { min: 1, max: 1024, step: 1 },
  tileRefreshMs: { min: 250, max: 10000, step: 50 },
  densityGain: { min: 0.1, max: 8.0, step: 0.1 },
  frameRateCap: { min: 15, max: 240, step: 1 },
} as const

export type NumericSetting = keyof typeof BOUNDS

export function clamp(v: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, v))
}

function num(v: unknown, fallback: number, min: number, max: number, integer: boolean): number {
  if (typeof v !== 'number' || !Number.isFinite(v)) return fallback
  const c = clamp(v, min, max)
  return integer ? Math.round(c) : c
}

export function sanitize(input: unknown): GraphicsSettings {
  const o = (typeof input === 'object' && input !== null ? input : {}) as Record<string, unknown>
  const b = BOUNDS
  return {
    liveChunkLimit: num(o.liveChunkLimit, DEFAULT_SETTINGS.liveChunkLimit, b.liveChunkLimit.min, b.liveChunkLimit.max, true),
    tileRefreshMs: num(o.tileRefreshMs, DEFAULT_SETTINGS.tileRefreshMs, b.tileRefreshMs.min, b.tileRefreshMs.max, true),
    densityGain: num(o.densityGain, DEFAULT_SETTINGS.densityGain, b.densityGain.min, b.densityGain.max, false),
    frameRateCap: num(o.frameRateCap, DEFAULT_SETTINGS.frameRateCap, b.frameRateCap.min, b.frameRateCap.max, true),
    prediction: typeof o.prediction === 'boolean' ? o.prediction : DEFAULT_SETTINGS.prediction,
  }
}

export function loadSettings(): GraphicsSettings {
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    if (!raw) return { ...DEFAULT_SETTINGS }
    return sanitize(JSON.parse(raw))
  } catch {
    return { ...DEFAULT_SETTINGS }
  }
}

export function saveSettings(s: GraphicsSettings): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(s))
  } catch {
    // storage unavailable
  }
}
