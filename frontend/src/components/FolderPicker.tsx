import { useEffect, useState } from 'react'
import { ChevronRight, File, Folder, FolderCheck, MoveUp, X } from 'lucide-react'
import { browseDir } from '../api'

interface Props { onSelect: (path: string) => void; onClose: () => void; initialPath?: string }

export default function FolderPicker({ onSelect, onClose, initialPath }: Props) {
  const [current, setCurrent] = useState(initialPath || '')
  const [items, setItems] = useState<any[]>([])
  const [parent, setParent] = useState('')
  const [loading, setLoading] = useState(false)
  const [inputPath, setInputPath] = useState(initialPath || '')
  const [error, setError] = useState('')

  const load = async (path: string) => {
    setLoading(true); setError('')
    try {
      const data = await browseDir(path)
      setCurrent(data.current); setItems(data.items || []); setParent(data.parent || ''); setInputPath(data.current)
    } catch (reason: any) {
      setItems([]); setError(reason.message || '无法打开目录')
    } finally { setLoading(false) }
  }
  useEffect(() => { load(current) }, [])

  return <div className="modal-overlay nested-modal" onMouseDown={onClose}><section className="modal folder-modal" onMouseDown={event => event.stopPropagation()}>
    <header><div><h2>选择下载目录</h2><p>{current || '此电脑'}</p></div><button className="icon-button" title="关闭" onClick={onClose}><X size={18} /></button></header>
    <div className="path-bar"><input value={inputPath} onChange={event => setInputPath(event.target.value)} onKeyDown={event => { if (event.key === 'Enter' && inputPath.trim()) load(inputPath.trim()) }} placeholder="输入路径" /><button className="secondary-button" onClick={() => inputPath.trim() && load(inputPath.trim())}>跳转</button><button className="icon-button bordered" title="上级目录" disabled={!parent} onClick={() => parent && load(parent)}><MoveUp size={17} /></button></div>
    <div className="folder-list">{loading ? <div className="folder-empty">加载中...</div> : items.length === 0 ? <div className="folder-empty">{error || '空目录'}</div> : items.map(item => <button key={item.path} className="folder-item" disabled={!item.is_dir} onDoubleClick={() => item.is_dir && load(item.path)} onClick={() => item.is_dir && load(item.path)}>{item.is_dir ? <Folder size={17} /> : <File size={17} />}<span title={item.path}>{item.name}</span>{item.is_dir && <ChevronRight size={15} />}</button>)}</div>
    <footer><button className="secondary-button" onClick={onClose}>取消</button><button className="primary-button" disabled={!current} onClick={() => onSelect(current)}><FolderCheck size={16} />选择此目录</button></footer>
  </section></div>
}
