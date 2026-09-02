import type { RendererStatus } from '../renderer'

interface Props {
  status: RendererStatus
}

export function StatusBar({ status }: Props) {
  const connected = status.connection === 'connected'
  const modeText = status.mode === 'live' ? 'live' : `tiles L${status.level}`
  const countText = status.mode === 'live' ? `${status.chunks} chunks` : `${status.tiles} tiles`

  return (
    <div className="status" role="status">
      <span className={`dot ${connected ? 'dot-on' : 'dot-off'}`} aria-hidden="true" />
      <span>{connected ? 'connected' : status.connection}</span>
      <span className="sep" />
      <span title="world id">{status.worldId || '—'}</span>
      <span className="sep" />
      <span>{modeText}</span>
      <span className="sep" />
      <span>{status.scale.toFixed(2)} px/cell</span>
      <span className="sep" />
      <span>tick {status.tick}</span>
      <span className="sep" />
      <span>{countText}</span>
      <span className="sep" />
      <span>{Math.round(status.fps)} fps</span>
    </div>
  )
}
