import { useEffect, useRef, useState } from 'react'
import { RefreshCw, X } from 'lucide-react'
import { connectSSE, fetchLog } from '../api'

export default function LogModal({ taskId, onClose }: { taskId: string; onClose: () => void }) {
  const [log, setLog] = useState('加载中...')
  const [autoScroll, setAutoScroll] = useState(true)
  const containerRef = useRef<HTMLPreElement | null>(null)
  const reload = () => fetchLog(taskId).then(data => setLog(data.log || '(empty)')).catch(reason => setLog(`加载日志失败: ${reason.message || reason}`))

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => { if (event.key === 'Escape') onClose() }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [onClose])
  useEffect(() => { void reload() }, [taskId])
  useEffect(() => {
    const events = connectSSE(event => {
      if (event.task_id !== taskId || !['task_log', 'task_progress'].includes(event.type)) return
      const line = event.message ? String(event.message) : ''
      if (line) setLog(previous => previous === '加载中...' ? line : `${previous}\n${line}`)
    })
    return () => events.close()
  }, [taskId])
  useEffect(() => { if (autoScroll && containerRef.current) containerRef.current.scrollTop = containerRef.current.scrollHeight }, [log, autoScroll])

  return <div className="modal-overlay" onMouseDown={onClose}><section className="modal log-modal" onMouseDown={event => event.stopPropagation()}>
    <header><div><h2>实时日志</h2><p>任务 {taskId}</p></div><div className="header-actions"><label className="checkbox-label"><input type="checkbox" checked={autoScroll} onChange={event => setAutoScroll(event.target.checked)} />自动滚动</label><button className="icon-button bordered" title="刷新日志" onClick={reload}><RefreshCw size={16} /></button><button className="icon-button" title="关闭" onClick={onClose}><X size={18} /></button></div></header>
    <pre ref={containerRef} className="log-output">{log}</pre>
  </section></div>
}
