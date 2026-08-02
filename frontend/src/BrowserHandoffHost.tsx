import { useEffect, useState } from 'react'
import BrowserHandoffWindow from './BrowserHandoffWindow'

export default function BrowserHandoffHost() {
  const [handoffId, setHandoffId] = useState('')

  useEffect(() => {
    let unlisten: (() => void) | undefined
    void import('@tauri-apps/api/event').then(async ({ emitTo, listen }) => {
      unlisten = await listen<{ id?: string }>('handoff-request', event => {
        setHandoffId(String(event.payload?.id || ''))
      })
      await emitTo('main', 'handoff-host-ready', {})
    })
    return () => unlisten?.()
  }, [])

  const resolved = (id: string) => {
    setHandoffId('')
    void import('@tauri-apps/api/event').then(({ emitTo }) =>
      emitTo('main', 'handoff-resolved', { id }),
    )
  }

  if (!handoffId) return <main className="handoff-window-root" aria-hidden="true" />
  return <BrowserHandoffWindow key={handoffId} handoffId={handoffId} persistent onClosed={resolved} />
}
