import { SUBSCRIBE_DEBOUNCE_MS } from './config'
import type { ChunkCoord, ClientMessage, ServerMessage } from './protocol'

export type ConnectionState = 'connecting' | 'connected' | 'reconnecting' | 'closed'

export interface GatewayHandlers {
  onMessage: (msg: ServerMessage) => void
  onStatus: (state: ConnectionState) => void
}

const BACKOFF_BASE_MS = 500
const BACKOFF_MAX_MS = 10000

export class Gateway {
  state: ConnectionState = 'connecting'

  private ws: WebSocket | null = null
  private desired: string | null = null
  private lastSent: string | null = null
  private lastSentAt = -Infinity
  private flushTimer: number | null = null
  private reconnectTimer: number | null = null
  private attempt = 0
  private closed = false

  constructor(
    private readonly url: string,
    private readonly handlers: GatewayHandlers,
  ) {}

  connect(): void {
    if (this.closed || this.ws) return
    let ws: WebSocket
    try {
      ws = new WebSocket(this.url)
    } catch {
      this.scheduleReconnect()
      return
    }
    this.ws = ws
    ws.onopen = () => {
      if (this.ws !== ws) return
      this.attempt = 0
      this.lastSent = null
      this.setState('connected')
      this.flush()
    }
    ws.onmessage = (ev) => {
      if (this.ws !== ws || typeof ev.data !== 'string') return
      let msg: ServerMessage
      try {
        msg = JSON.parse(ev.data) as ServerMessage
      } catch {
        return
      }
      this.handlers.onMessage(msg)
    }
    ws.onerror = () => {
      if (this.ws === ws) ws.close()
    }
    ws.onclose = () => {
      if (this.ws !== ws) return
      this.ws = null
      this.lastSent = null
      this.scheduleReconnect()
    }
  }

  subscribe(chunks: ChunkCoord[]): void {
    const key = JSON.stringify(chunks)
    if (key === this.desired) return
    this.desired = key
    this.scheduleFlush()
  }

  close(): void {
    this.closed = true
    if (this.flushTimer !== null) window.clearTimeout(this.flushTimer)
    if (this.reconnectTimer !== null) window.clearTimeout(this.reconnectTimer)
    this.flushTimer = null
    this.reconnectTimer = null
    const ws = this.ws
    this.ws = null
    if (ws) {
      ws.onclose = null
      ws.onerror = null
      ws.onmessage = null
      ws.onopen = null
      ws.close()
    }
    this.setState('closed')
  }

  private setState(s: ConnectionState): void {
    if (this.state === s) return
    this.state = s
    this.handlers.onStatus(s)
  }

  private scheduleReconnect(): void {
    if (this.closed || this.reconnectTimer !== null) return
    this.setState('reconnecting')
    const delay = Math.min(BACKOFF_MAX_MS, BACKOFF_BASE_MS * 2 ** this.attempt) * (0.8 + Math.random() * 0.4)
    this.attempt = Math.min(this.attempt + 1, 8)
    this.reconnectTimer = window.setTimeout(() => {
      this.reconnectTimer = null
      this.connect()
    }, delay)
  }

  private scheduleFlush(): void {
    if (this.flushTimer !== null) return
    const elapsed = performance.now() - this.lastSentAt
    if (elapsed >= SUBSCRIBE_DEBOUNCE_MS) {
      this.flush()
      return
    }
    this.flushTimer = window.setTimeout(() => {
      this.flushTimer = null
      this.flush()
    }, SUBSCRIBE_DEBOUNCE_MS - elapsed)
  }

  private flush(): void {
    const ws = this.ws
    if (!ws || ws.readyState !== WebSocket.OPEN) return
    if (this.desired === null || this.desired === this.lastSent) return
    const msg: ClientMessage = { type: 'subscribe', chunks: JSON.parse(this.desired) as ChunkCoord[] }
    ws.send(JSON.stringify(msg))
    this.lastSent = this.desired
    this.lastSentAt = performance.now()
  }
}
