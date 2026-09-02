import { Application, Container } from 'pixi.js'
import { ChunkLayer } from './chunkLayer'
import { GATEWAY_URL, STATUS_INTERVAL_MS, WS_URL } from './config'
import { Gateway, type ConnectionState } from './gateway'
import type { ServerMessage, WorldInfo } from './protocol'
import { DEFAULT_SETTINGS, clamp, type GraphicsSettings } from './settings'
import { TileLayer } from './tileLayer'
import { Viewport, rangeCount } from './viewport'

export type ViewMode = 'live' | 'tiles'

export interface RendererStatus {
  worldId: string
  mode: ViewMode
  level: number
  scale: number
  tick: number
  chunks: number
  tiles: number
  fps: number
  connection: ConnectionState
}

export const INITIAL_STATUS: RendererStatus = {
  worldId: '',
  mode: 'tiles',
  level: 1,
  scale: 1,
  tick: 0,
  chunks: 0,
  tiles: 0,
  fps: 0,
  connection: 'connecting',
}

interface PointerPos {
  x: number
  y: number
}

const WORLD_RETRY_MAX_MS = 10000

export class Renderer {
  prediction = DEFAULT_SETTINGS.prediction

  private readonly app = new Application()
  private readonly world = new Container()
  private readonly viewport = new Viewport()
  private readonly chunks = new ChunkLayer()
  private readonly tiles = new TileLayer()
  private readonly gateway: Gateway
  private readonly resizeObserver: ResizeObserver
  private readonly pointers = new Map<number, PointerPos>()
  private readonly unbind: Array<() => void> = []

  private settings: GraphicsSettings = { ...DEFAULT_SETTINGS }
  private destroyed = false
  private started = false
  private raf = 0
  private lastFrameAt = -Infinity
  private lastStatusAt = 0
  private fpsWindowStart = 0
  private fpsFrames = 0
  private fps = 0
  private worldId = ''
  private summaryLevels = 1
  private tick = 0
  private mode: ViewMode = 'tiles'
  private level = 1
  private tileStamp = Date.now()
  private tileStampAt = -Infinity
  private pinchDist = 0
  private pinchMid: PointerPos = { x: 0, y: 0 }
  private worldRetry = 0
  private worldTimer: number | null = null

  constructor(
    private readonly host: HTMLElement,
    private readonly onStatus: (status: RendererStatus) => void,
  ) {
    this.gateway = new Gateway(WS_URL, {
      onMessage: (msg) => this.handleMessage(msg),
      onStatus: () => this.emitStatus(),
    })
    this.resizeObserver = new ResizeObserver(() => this.applySize())
  }

  async start(): Promise<void> {
    await this.app.init({
      width: Math.max(1, this.host.clientWidth),
      height: Math.max(1, this.host.clientHeight),
      background: 0xffffff,
      backgroundAlpha: 1,
      antialias: false,
      resolution: window.devicePixelRatio || 1,
      autoDensity: true,
      autoStart: false,
      sharedTicker: false,
      preference: 'webgl',
    })
    if (this.destroyed) {
      this.app.destroy(true)
      return
    }
    this.started = true
    this.host.appendChild(this.app.canvas)
    this.app.stage.addChild(this.world)
    this.world.addChild(this.tiles.container, this.chunks.container)
    this.bindInput()
    this.resizeObserver.observe(this.host)
    this.applySize()
    this.gateway.connect()
    void this.loadWorldInfo()
    this.raf = requestAnimationFrame(this.frame)
  }

  setSettings(s: GraphicsSettings): void {
    this.settings = { ...s }
    this.prediction = s.prediction
    this.tiles.setGain(s.densityGain)
  }

  destroy(): void {
    if (this.destroyed) return
    this.destroyed = true
    cancelAnimationFrame(this.raf)
    if (this.worldTimer !== null) window.clearTimeout(this.worldTimer)
    this.resizeObserver.disconnect()
    this.gateway.close()
    for (const off of this.unbind) off()
    this.unbind.length = 0
    if (this.started) {
      this.chunks.clear()
      this.tiles.clear()
      this.app.destroy(true, { children: true })
    }
  }

  private async loadWorldInfo(): Promise<void> {
    try {
      const res = await fetch(`${GATEWAY_URL}/world`)
      if (!res.ok) throw new Error(`world ${res.status}`)
      const info = (await res.json()) as WorldInfo
      if (this.destroyed) return
      this.configureWorld(info)
      this.worldRetry = 0
    } catch {
      if (this.destroyed) return
      const delay = Math.min(WORLD_RETRY_MAX_MS, 500 * 2 ** this.worldRetry)
      this.worldRetry = Math.min(this.worldRetry + 1, 8)
      this.worldTimer = window.setTimeout(() => {
        this.worldTimer = null
        void this.loadWorldInfo()
      }, delay)
    }
  }

  private configureWorld(info: WorldInfo): void {
    const { map_width, map_height } = info.config
    const changed =
      this.worldId !== info.world_id || this.viewport.mapW !== map_width || this.viewport.mapH !== map_height
    this.summaryLevels = Math.max(1, info.summary_levels)
    if (!changed) return
    this.worldId = info.world_id
    this.tick = 0
    this.chunks.clear()
    this.tiles.clear()
    this.viewport.setMap(map_width, map_height)
    this.emitStatus()
  }

  private handleMessage(msg: ServerMessage): void {
    switch (msg.type) {
      case 'world':
        this.configureWorld(msg)
        break
      case 'chunk':
        this.chunks.applyState(msg)
        break
      case 'events':
        this.chunks.applyEvents(msg)
        break
      default:
        return
    }
    if (typeof msg.tick === 'number' && msg.tick > this.tick) this.tick = msg.tick
  }

  private applySize(): void {
    const w = Math.max(1, this.host.clientWidth)
    const h = Math.max(1, this.host.clientHeight)
    if (w === this.viewport.width && h === this.viewport.height) return
    this.app.renderer.resize(w, h)
    this.viewport.resize(w, h)
  }

  private bindInput(): void {
    const canvas = this.app.canvas
    const local = (e: PointerEvent | WheelEvent): PointerPos => {
      const rect = canvas.getBoundingClientRect()
      return { x: e.clientX - rect.left, y: e.clientY - rect.top }
    }
    const updatePinch = (): void => {
      const [a, b] = [...this.pointers.values()]
      if (!a || !b) return
      this.pinchDist = Math.hypot(b.x - a.x, b.y - a.y)
      this.pinchMid = { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 }
    }
    const onDown = (e: PointerEvent): void => {
      canvas.setPointerCapture(e.pointerId)
      this.pointers.set(e.pointerId, local(e))
      if (this.pointers.size === 2) updatePinch()
    }
    const onMove = (e: PointerEvent): void => {
      const prev = this.pointers.get(e.pointerId)
      if (!prev) return
      const cur = local(e)
      this.pointers.set(e.pointerId, cur)
      if (this.pointers.size === 1) {
        this.viewport.panBy(cur.x - prev.x, cur.y - prev.y)
      } else if (this.pointers.size === 2) {
        const [a, b] = [...this.pointers.values()]
        if (!a || !b) return
        const dist = Math.hypot(b.x - a.x, b.y - a.y)
        const mid = { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 }
        if (this.pinchDist > 0 && dist > 0) this.viewport.zoomAt(mid.x, mid.y, dist / this.pinchDist)
        this.viewport.panBy(mid.x - this.pinchMid.x, mid.y - this.pinchMid.y)
        this.pinchDist = dist
        this.pinchMid = mid
      }
    }
    const onUp = (e: PointerEvent): void => {
      this.pointers.delete(e.pointerId)
      if (canvas.hasPointerCapture(e.pointerId)) canvas.releasePointerCapture(e.pointerId)
      if (this.pointers.size === 2) updatePinch()
    }
    const onWheel = (e: WheelEvent): void => {
      e.preventDefault()
      const dy = e.deltaMode === 1 ? e.deltaY * 16 : e.deltaMode === 2 ? e.deltaY * 100 : e.deltaY
      const rate = e.ctrlKey ? 0.01 : 0.002
      const p = local(e)
      this.viewport.zoomAt(p.x, p.y, Math.exp(-dy * rate))
    }
    canvas.addEventListener('pointerdown', onDown)
    canvas.addEventListener('pointermove', onMove)
    canvas.addEventListener('pointerup', onUp)
    canvas.addEventListener('pointercancel', onUp)
    canvas.addEventListener('wheel', onWheel, { passive: false })
    this.unbind.push(
      () => canvas.removeEventListener('pointerdown', onDown),
      () => canvas.removeEventListener('pointermove', onMove),
      () => canvas.removeEventListener('pointerup', onUp),
      () => canvas.removeEventListener('pointercancel', onUp),
      () => canvas.removeEventListener('wheel', onWheel),
    )
  }

  private readonly frame = (now: number): void => {
    if (this.destroyed) return
    this.raf = requestAnimationFrame(this.frame)
    const minInterval = 1000 / this.settings.frameRateCap
    if (now - this.lastFrameAt < minInterval * 0.9) return
    this.lastFrameAt = now

    this.fpsFrames++
    const fpsElapsed = now - this.fpsWindowStart
    if (fpsElapsed >= 1000) {
      this.fps = (this.fpsFrames * 1000) / fpsElapsed
      this.fpsFrames = 0
      this.fpsWindowStart = now
    }

    this.update(now)
    this.app.render()

    if (now - this.lastStatusAt >= STATUS_INTERVAL_MS) {
      this.lastStatusAt = now
      this.emitStatus()
    }
  }

  private update(now: number): void {
    const vp = this.viewport
    if (!vp.hasMap) return
    const chunkRange = vp.chunkRange()
    const live = vp.s >= 1 && rangeCount(chunkRange) <= this.settings.liveChunkLimit

    if (live) {
      this.mode = 'live'
      this.tiles.container.visible = false
      this.tiles.clear()
      this.chunks.container.visible = true
      this.gateway.subscribe(this.chunks.setVisible(chunkRange))
    } else {
      this.mode = 'tiles'
      this.level = vp.s < 1 ? clamp(Math.ceil(Math.log2(1 / vp.s)), 1, this.summaryLevels) : 1
      this.chunks.container.visible = false
      this.chunks.clear()
      this.gateway.subscribe([])
      if (now - this.tileStampAt >= this.settings.tileRefreshMs) {
        this.tileStamp = Date.now()
        this.tileStampAt = now
      }
      this.tiles.container.visible = true
      this.tiles.update(this.worldId, this.level, vp.tileRange(this.level), this.tileStamp)
    }

    this.world.position.set(vp.panX, vp.panY)
    this.world.scale.set(vp.s)
    this.chunks.flush()
  }

  private emitStatus(): void {
    if (this.destroyed) return
    this.onStatus({
      worldId: this.worldId,
      mode: this.mode,
      level: this.level,
      scale: this.viewport.s,
      tick: this.tick,
      chunks: this.chunks.count,
      tiles: this.tiles.loadedCount,
      fps: this.fps,
      connection: this.gateway.state,
    })
  }
}
