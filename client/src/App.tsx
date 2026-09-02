import { useEffect, useRef, useState } from 'react'
import { SettingsPanel } from './components/SettingsPanel'
import { StatusBar } from './components/StatusBar'
import { INITIAL_STATUS, Renderer, type RendererStatus } from './renderer'
import { loadSettings, saveSettings, type GraphicsSettings } from './settings'

export function App() {
  const hostRef = useRef<HTMLDivElement>(null)
  const rendererRef = useRef<Renderer | null>(null)
  const settingsRef = useRef<GraphicsSettings>(loadSettings())
  const [settings, setSettings] = useState<GraphicsSettings>(settingsRef.current)
  const [status, setStatus] = useState<RendererStatus>(INITIAL_STATUS)
  const [open, setOpen] = useState(false)

  useEffect(() => {
    const host = hostRef.current
    if (!host) return
    const renderer = new Renderer(host, setStatus)
    renderer.setSettings(settingsRef.current)
    rendererRef.current = renderer
    void renderer.start()
    return () => {
      rendererRef.current = null
      renderer.destroy()
    }
  }, [])

  useEffect(() => {
    settingsRef.current = settings
    rendererRef.current?.setSettings(settings)
    saveSettings(settings)
  }, [settings])

  return (
    <>
      <div ref={hostRef} className="canvas-host" />
      <SettingsPanel settings={settings} onChange={setSettings} open={open} onToggle={() => setOpen((o) => !o)} />
      <StatusBar status={status} />
    </>
  )
}
