import { useEffect, useState } from 'react'
import { BOUNDS, DEFAULT_SETTINGS, clamp, type GraphicsSettings, type NumericSetting } from '../settings'

interface Props {
  settings: GraphicsSettings
  onChange: (next: GraphicsSettings) => void
  open: boolean
  onToggle: () => void
}

interface NumberFieldProps {
  id: NumericSetting
  label: string
  value: number
  onCommit: (v: number) => void
}

function NumberField({ id, label, value, onCommit }: NumberFieldProps) {
  const bounds = BOUNDS[id]
  const [draft, setDraft] = useState(String(value))
  useEffect(() => setDraft(String(value)), [value])

  const commit = (): void => {
    const parsed = Number(draft)
    if (!Number.isFinite(parsed)) {
      setDraft(String(value))
      return
    }
    onCommit(Math.round(clamp(parsed, bounds.min, bounds.max)))
  }

  return (
    <label className="field">
      <span className="field-label">{label}</span>
      <input
        type="number"
        min={bounds.min}
        max={bounds.max}
        step={bounds.step}
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === 'Enter') (e.target as HTMLInputElement).blur()
        }}
      />
    </label>
  )
}

export function SettingsPanel({ settings, onChange, open, onToggle }: Props) {
  const set = <K extends keyof GraphicsSettings>(key: K, value: GraphicsSettings[K]): void => {
    onChange({ ...settings, [key]: value })
  }
  const gain = BOUNDS.densityGain

  return (
    <div className="settings">
      <button
        type="button"
        className="gear"
        aria-label={open ? 'Close graphics settings' : 'Open graphics settings'}
        aria-expanded={open}
        onClick={onToggle}
      >
        <svg viewBox="0 0 24 24" width="20" height="20" aria-hidden="true">
          <path
            fill="currentColor"
            d="M19.14 12.94a7.07 7.07 0 0 0 .06-.94 7.07 7.07 0 0 0-.06-.94l2.03-1.58a.5.5 0 0 0 .12-.64l-1.92-3.32a.5.5 0 0 0-.61-.22l-2.39.96a7.3 7.3 0 0 0-1.63-.94l-.36-2.54A.5.5 0 0 0 13.9 2h-3.8a.5.5 0 0 0-.49.42l-.36 2.54c-.59.24-1.13.56-1.63.94l-2.39-.96a.5.5 0 0 0-.61.22L2.7 8.48a.5.5 0 0 0 .12.64l2.03 1.58a7.07 7.07 0 0 0 0 1.88l-2.03 1.58a.5.5 0 0 0-.12.64l1.92 3.32c.13.22.39.31.61.22l2.39-.96c.5.38 1.04.7 1.63.94l.36 2.54c.04.24.25.42.49.42h3.8c.24 0 .45-.18.49-.42l.36-2.54c.59-.24 1.13-.56 1.63-.94l2.39.96c.22.09.48 0 .61-.22l1.92-3.32a.5.5 0 0 0-.12-.64l-2.03-1.58ZM12 15.5A3.5 3.5 0 1 1 12 8.5a3.5 3.5 0 0 1 0 7Z"
          />
        </svg>
      </button>
      {open && (
        <div className="panel" role="dialog" aria-label="Graphics settings">
          <h2>Graphics</h2>
          <NumberField
            id="liveChunkLimit"
            label="Live chunk limit"
            value={settings.liveChunkLimit}
            onCommit={(v) => set('liveChunkLimit', v)}
          />
          <NumberField
            id="tileRefreshMs"
            label="Tile refresh (ms)"
            value={settings.tileRefreshMs}
            onCommit={(v) => set('tileRefreshMs', v)}
          />
          <label className="field">
            <span className="field-label">
              Density gain <span className="field-value">{settings.densityGain.toFixed(1)}</span>
            </span>
            <input
              type="range"
              min={gain.min}
              max={gain.max}
              step={gain.step}
              value={settings.densityGain}
              onChange={(e) => set('densityGain', clamp(Number(e.target.value), gain.min, gain.max))}
            />
          </label>
          <NumberField
            id="frameRateCap"
            label="Frame rate cap"
            value={settings.frameRateCap}
            onCommit={(v) => set('frameRateCap', v)}
          />
          <label className="field field-row">
            <span className="field-label">Prediction</span>
            <input
              type="checkbox"
              checked={settings.prediction}
              onChange={(e) => set('prediction', e.target.checked)}
            />
          </label>
          <button type="button" className="reset" onClick={() => onChange({ ...DEFAULT_SETTINGS })}>
            Reset to defaults
          </button>
        </div>
      )}
    </div>
  )
}
