import { CHUNK_SIZE, MAX_ZOOM, TILE_SIZE } from './config'

export interface GridRange {
  x0: number
  y0: number
  x1: number
  y1: number
}

export function rangeCount(r: GridRange): number {
  return Math.max(0, r.x1 - r.x0) * Math.max(0, r.y1 - r.y0)
}

export function tileCells(level: number): number {
  return TILE_SIZE * 2 ** level
}

export class Viewport {
  s = 1
  panX = 0
  panY = 0
  width = 1
  height = 1
  mapW = 0
  mapH = 0

  get hasMap(): boolean {
    return this.mapW > 0 && this.mapH > 0
  }

  sMin(): number {
    if (!this.hasMap) return 1
    return Math.min(MAX_ZOOM, this.width / this.mapW, this.height / this.mapH)
  }

  setMap(w: number, h: number): void {
    this.mapW = w
    this.mapH = h
    this.fit()
  }

  fit(): void {
    this.s = this.sMin()
    this.panX = (this.width - this.mapW * this.s) / 2
    this.panY = (this.height - this.mapH * this.s) / 2
  }

  resize(w: number, h: number): void {
    const cx = (this.width / 2 - this.panX) / this.s
    const cy = (this.height / 2 - this.panY) / this.s
    this.width = Math.max(1, w)
    this.height = Math.max(1, h)
    if (!this.hasMap) return
    this.s = Math.min(MAX_ZOOM, Math.max(this.sMin(), this.s))
    this.panX = this.width / 2 - cx * this.s
    this.panY = this.height / 2 - cy * this.s
    this.clampPan()
  }

  zoomAt(sx: number, sy: number, factor: number): void {
    if (!this.hasMap) return
    const next = Math.min(MAX_ZOOM, Math.max(this.sMin(), this.s * factor))
    if (next === this.s) return
    const cx = (sx - this.panX) / this.s
    const cy = (sy - this.panY) / this.s
    this.s = next
    this.panX = sx - cx * next
    this.panY = sy - cy * next
    this.clampPan()
  }

  panBy(dx: number, dy: number): void {
    this.panX += dx
    this.panY += dy
    this.clampPan()
  }

  clampPan(): void {
    if (!this.hasMap) return
    const halfW = this.width / 2
    const halfH = this.height / 2
    const mapPxW = this.mapW * this.s
    const mapPxH = this.mapH * this.s
    this.panX = Math.min(halfW, Math.max(halfW - mapPxW, this.panX))
    this.panY = Math.min(halfH, Math.max(halfH - mapPxH, this.panY))
  }

  visibleCells(): GridRange {
    const x0 = Math.max(0, Math.floor(-this.panX / this.s))
    const y0 = Math.max(0, Math.floor(-this.panY / this.s))
    const x1 = Math.min(this.mapW, Math.ceil((this.width - this.panX) / this.s))
    const y1 = Math.min(this.mapH, Math.ceil((this.height - this.panY) / this.s))
    return { x0, y0, x1: Math.max(x0, x1), y1: Math.max(y0, y1) }
  }

  private gridRange(cellsPer: number): GridRange {
    const v = this.visibleCells()
    if (v.x1 <= v.x0 || v.y1 <= v.y0) return { x0: 0, y0: 0, x1: 0, y1: 0 }
    const gx = Math.ceil(this.mapW / cellsPer)
    const gy = Math.ceil(this.mapH / cellsPer)
    return {
      x0: Math.max(0, Math.floor(v.x0 / cellsPer)),
      y0: Math.max(0, Math.floor(v.y0 / cellsPer)),
      x1: Math.min(gx, Math.ceil(v.x1 / cellsPer)),
      y1: Math.min(gy, Math.ceil(v.y1 / cellsPer)),
    }
  }

  chunkRange(): GridRange {
    return this.gridRange(CHUNK_SIZE)
  }

  tileRange(level: number): GridRange {
    return this.gridRange(tileCells(level))
  }
}
