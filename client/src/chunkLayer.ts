import { CanvasSource, Container, Sprite, Texture } from 'pixi.js'
import { CHUNK_SIZE } from './config'
import type { ChunkCoord, ChunkMessage, EventsMessage, Rgba } from './protocol'
import type { GridRange } from './viewport'

const UNKNOWN_COLOR: Rgba = [128, 128, 128, 255]
const WHITE: Rgba = [255, 255, 255, 255]

interface ChunkEntry {
  cx: number
  cy: number
  ctx: CanvasRenderingContext2D
  image: ImageData
  texture: Texture
  sprite: Sprite
  colors: Map<number, Rgba>
  terrain: Map<number, Rgba>
  occupied: Uint8Array
  dirty: boolean
}

function key(cx: number, cy: number): string {
  return `${cx},${cy}`
}

export class ChunkLayer {
  readonly container = new Container()
  private readonly entries = new Map<string, ChunkEntry>()

  get count(): number {
    return this.entries.size
  }

  setVisible(range: GridRange): ChunkCoord[] {
    const wanted = new Set<string>()
    const list: ChunkCoord[] = []
    for (let cy = range.y0; cy < range.y1; cy++) {
      for (let cx = range.x0; cx < range.x1; cx++) {
        const k = key(cx, cy)
        wanted.add(k)
        list.push([cx, cy])
        if (!this.entries.has(k)) this.entries.set(k, this.create(cx, cy))
      }
    }
    for (const [k, entry] of this.entries) {
      if (!wanted.has(k)) {
        this.dispose(entry)
        this.entries.delete(k)
      }
    }
    return list
  }

  applyState(msg: ChunkMessage): void {
    const entry = this.entries.get(key(msg.cx, msg.cy))
    if (!entry) return
    entry.image.data.fill(255)
    entry.colors.clear()
    entry.terrain.clear()
    entry.occupied.fill(0)
    for (const c of msg.cells ?? []) {
      const i = this.index(entry, c.x, c.y)
      if (i < 0 || !c.color) continue
      entry.terrain.set(i, c.color)
      this.paint(entry, i, c.color)
    }
    for (const p of msg.pixels) {
      entry.colors.set(p.id, p.color)
      this.placePixel(entry, p.x, p.y, p.color)
    }
    entry.dirty = true
  }

  applyEvents(msg: EventsMessage): void {
    const entry = this.entries.get(key(msg.cx, msg.cy))
    if (!entry) return
    for (const ev of msg.events) {
      if (ev.t === 'moved' || ev.t === 'departed') {
        this.vacate(entry, ev.from[0], ev.from[1])
        if (ev.t === 'departed') entry.colors.delete(ev.id)
      }
    }
    for (const ev of msg.events) {
      switch (ev.t) {
        case 'placed':
        case 'arrived':
          entry.colors.set(ev.id, ev.color)
          this.placePixel(entry, ev.x, ev.y, ev.color)
          break
        case 'moved':
          this.placePixel(entry, ev.to[0], ev.to[1], entry.colors.get(ev.id) ?? UNKNOWN_COLOR)
          break
        case 'cell_set': {
          const i = this.index(entry, ev.x, ev.y)
          if (i < 0) break
          if (ev.color) entry.terrain.set(i, ev.color)
          else entry.terrain.delete(i)
          if (!entry.occupied[i]) this.paint(entry, i, ev.color ?? WHITE)
          break
        }
        default:
          break
      }
    }
    entry.dirty = true
  }

  flush(): void {
    for (const entry of this.entries.values()) {
      if (!entry.dirty) continue
      entry.ctx.putImageData(entry.image, 0, 0)
      entry.texture.source.update()
      entry.dirty = false
    }
  }

  clear(): void {
    for (const entry of this.entries.values()) this.dispose(entry)
    this.entries.clear()
  }

  private create(cx: number, cy: number): ChunkEntry {
    const canvas = document.createElement('canvas')
    canvas.width = CHUNK_SIZE
    canvas.height = CHUNK_SIZE
    const ctx = canvas.getContext('2d')
    if (!ctx) throw new Error('2d context unavailable')
    const image = ctx.createImageData(CHUNK_SIZE, CHUNK_SIZE)
    image.data.fill(255)
    ctx.putImageData(image, 0, 0)
    const texture = new Texture({ source: new CanvasSource({ resource: canvas, scaleMode: 'nearest' }) })
    const sprite = new Sprite(texture)
    sprite.position.set(cx * CHUNK_SIZE, cy * CHUNK_SIZE)
    this.container.addChild(sprite)
    return {
      cx,
      cy,
      ctx,
      image,
      texture,
      sprite,
      colors: new Map(),
      terrain: new Map(),
      occupied: new Uint8Array(CHUNK_SIZE * CHUNK_SIZE),
      dirty: false,
    }
  }

  private dispose(entry: ChunkEntry): void {
    entry.sprite.destroy()
    entry.texture.destroy(true)
  }

  private index(entry: ChunkEntry, x: number, y: number): number {
    const lx = x - entry.cx * CHUNK_SIZE
    const ly = y - entry.cy * CHUNK_SIZE
    if (lx < 0 || ly < 0 || lx >= CHUNK_SIZE || ly >= CHUNK_SIZE) return -1
    return ly * CHUNK_SIZE + lx
  }

  private paint(entry: ChunkEntry, i: number, color: Rgba): void {
    const o = i * 4
    const d = entry.image.data
    d[o] = color[0]
    d[o + 1] = color[1]
    d[o + 2] = color[2]
    d[o + 3] = color[3]
  }

  private placePixel(entry: ChunkEntry, x: number, y: number, color: Rgba): void {
    const i = this.index(entry, x, y)
    if (i < 0) return
    entry.occupied[i] = 1
    this.paint(entry, i, color)
  }

  private vacate(entry: ChunkEntry, x: number, y: number): void {
    const i = this.index(entry, x, y)
    if (i < 0) return
    entry.occupied[i] = 0
    this.paint(entry, i, entry.terrain.get(i) ?? WHITE)
  }
}
