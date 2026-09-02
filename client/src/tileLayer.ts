import { CanvasSource, Container, Sprite, Texture } from 'pixi.js'
import { TILE_BASE_URL, TILE_SIZE } from './config'
import { tileCells, type GridRange } from './viewport'

type TileState = 'loading' | 'ready' | 'empty' | 'error'

interface TileEntry {
  level: number
  tx: number
  ty: number
  stamp: number
  state: TileState
  raw: Uint8ClampedArray | null
  ctx: CanvasRenderingContext2D | null
  image: ImageData | null
  texture: Texture | null
  sprite: Sprite | null
  abort: AbortController
  disposed: boolean
}

interface Slot {
  shown: TileEntry | null
  pending: TileEntry | null
}

const scratch = document.createElement('canvas')
scratch.width = TILE_SIZE
scratch.height = TILE_SIZE
const scratchCtx = scratch.getContext('2d', { willReadFrequently: true })

async function decodeTile(url: string, signal: AbortSignal): Promise<Uint8ClampedArray | null> {
  const res = await fetch(url, { signal, cache: 'no-store' })
  if (res.status === 404) return null
  if (!res.ok) throw new Error(`tile ${res.status}`)
  const blob = await res.blob()
  const bitmap = await createImageBitmap(blob, { premultiplyAlpha: 'none', colorSpaceConversion: 'none' })
  if (!scratchCtx) throw new Error('2d context unavailable')
  scratchCtx.clearRect(0, 0, TILE_SIZE, TILE_SIZE)
  scratchCtx.drawImage(bitmap, 0, 0, TILE_SIZE, TILE_SIZE)
  bitmap.close()
  return new Uint8ClampedArray(scratchCtx.getImageData(0, 0, TILE_SIZE, TILE_SIZE).data)
}

function applyGain(raw: Uint8ClampedArray, gain: number, out: Uint8ClampedArray): void {
  out.set(raw)
  for (let i = 3; i < raw.length; i += 4) {
    out[i] = Math.min(255, Math.round((raw[i] ?? 0) * gain))
  }
}

function posKey(level: number, tx: number, ty: number): string {
  return `${level}/${tx}/${ty}`
}

export class TileLayer {
  readonly container = new Container()
  private readonly slots = new Map<string, Slot>()
  private gain = 1

  get loadedCount(): number {
    let n = 0
    for (const slot of this.slots.values()) if (slot.shown) n++
    return n
  }

  setGain(gain: number): void {
    if (gain === this.gain) return
    this.gain = gain
    for (const slot of this.slots.values()) {
      const e = slot.shown
      if (e && e.state === 'ready' && e.raw && e.ctx && e.image && e.texture) {
        applyGain(e.raw, gain, e.image.data)
        e.ctx.putImageData(e.image, 0, 0)
        e.texture.source.update()
      }
    }
  }

  update(worldId: string, level: number, range: GridRange, stamp: number): void {
    const wanted = new Set<string>()
    for (let ty = range.y0; ty < range.y1; ty++) {
      for (let tx = range.x0; tx < range.x1; tx++) {
        const k = posKey(level, tx, ty)
        wanted.add(k)
        let slot = this.slots.get(k)
        if (!slot) {
          slot = { shown: null, pending: null }
          this.slots.set(k, slot)
        }
        const latest = slot.pending ?? slot.shown
        if (!latest || latest.stamp !== stamp) {
          if (slot.pending) this.dispose(slot.pending)
          slot.pending = this.load(worldId, level, tx, ty, stamp, slot)
        }
      }
    }
    for (const [k, slot] of this.slots) {
      if (wanted.has(k)) continue
      if (slot.shown) this.dispose(slot.shown)
      if (slot.pending) this.dispose(slot.pending)
      this.slots.delete(k)
    }
  }

  clear(): void {
    for (const slot of this.slots.values()) {
      if (slot.shown) this.dispose(slot.shown)
      if (slot.pending) this.dispose(slot.pending)
    }
    this.slots.clear()
  }

  private load(worldId: string, level: number, tx: number, ty: number, stamp: number, slot: Slot): TileEntry {
    const entry: TileEntry = {
      level,
      tx,
      ty,
      stamp,
      state: 'loading',
      raw: null,
      ctx: null,
      image: null,
      texture: null,
      sprite: null,
      abort: new AbortController(),
      disposed: false,
    }
    const url = `${TILE_BASE_URL}/tiles/${encodeURIComponent(worldId)}/${level}/${tx}/${ty}.png?v=${stamp}`
    decodeTile(url, entry.abort.signal).then(
      (raw) => {
        if (entry.disposed) return
        if (raw) {
          entry.raw = raw
          this.build(entry)
          entry.state = 'ready'
        } else {
          entry.state = 'empty'
        }
        if (slot.shown) this.dispose(slot.shown)
        slot.shown = entry
        slot.pending = null
      },
      () => {
        if (!entry.disposed) entry.state = 'error'
      },
    )
    return entry
  }

  private build(entry: TileEntry): void {
    if (!entry.raw) return
    const canvas = document.createElement('canvas')
    canvas.width = TILE_SIZE
    canvas.height = TILE_SIZE
    const ctx = canvas.getContext('2d')
    if (!ctx) throw new Error('2d context unavailable')
    const image = ctx.createImageData(TILE_SIZE, TILE_SIZE)
    applyGain(entry.raw, this.gain, image.data)
    ctx.putImageData(image, 0, 0)
    const texture = new Texture({ source: new CanvasSource({ resource: canvas, scaleMode: 'nearest' }) })
    const sprite = new Sprite(texture)
    const cells = tileCells(entry.level)
    sprite.position.set(entry.tx * cells, entry.ty * cells)
    sprite.scale.set(2 ** entry.level)
    this.container.addChild(sprite)
    entry.ctx = ctx
    entry.image = image
    entry.texture = texture
    entry.sprite = sprite
  }

  private dispose(entry: TileEntry): void {
    entry.disposed = true
    entry.abort.abort()
    entry.sprite?.destroy()
    entry.texture?.destroy(true)
    entry.sprite = null
    entry.texture = null
    entry.raw = null
  }
}
