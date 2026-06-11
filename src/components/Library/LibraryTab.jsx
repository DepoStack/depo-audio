import { useState, useMemo } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { Search, Download, ChevronDown, ChevronRight, Pencil, Archive, RotateCcw, X, Briefcase } from 'lucide-react'
import { cn } from '../../lib/utils'
import { CH_COLORS } from '../../constants'
import { usePreferencesContext } from '../../hooks/PreferencesContext'
import { Button } from '../ui/button'
import { Input } from '../ui/input'
import { Badge } from '../ui/badge'
import { Card } from '../ui/card'
import LibraryFile from './LibraryFile'
import ImportModal from './ImportModal'

export default function LibraryTab({ cases, setCases, search, setSearch, labels, onReexport }) {
  const { maxScanDepth } = usePreferencesContext()
  const [showArchived, setShowArchived] = useState(false)
  const [expandedCases, setExpandedCases]   = useState({})
  const [editingCase, setEditingCase]       = useState(null)
  const [editName, setEditName]             = useState('')
  const [importModal, setImportModal]       = useState(false)
  const [catSoftware, setCatSoftware]       = useState(null)
  const [catJobs, setCatJobs]               = useState([])
  const [scanningCat, setScanningCat]       = useState(false)

  const detectSoftware = async () => {
    setScanningCat(true)
    try {
      const sw = await invoke('detect_cat_software_cmd', { maxDepth: maxScanDepth })
      setCatSoftware(sw)
      // Auto-scan jobs from first detected software
      if (sw.length > 0) {
        const jobs = await invoke('scan_cat_jobs_cmd', { path: sw[0].path })
        setCatJobs(jobs)
      }
    } catch (e) {
      console.error('CAT detection failed:', e)
      setCatSoftware([])
    }
    setScanningCat(false)
  }

  const filtered = useMemo(() => cases
    .filter(c => showArchived ? c.archived : !c.archived)
    .filter(c => {
      if (!search.trim()) return true
      const q = search.toLowerCase()
      return c.name.toLowerCase().includes(q) ||
        c.sessions.some(s => s.sourceName?.toLowerCase().includes(q) ||
          s.participants.some(p => p.label.toLowerCase().includes(q)))
    }), [cases, showArchived, search])

  const toggleCase = (id) => setExpandedCases(p => ({...p, [id]: !p[id]}))

  const deleteCase = async (id) => {
    if (!confirm('Delete this case and all its session records? (Files on disk are not deleted.)')) return
    try {
      await invoke('library_delete_case', { caseId: id })
      setCases(p => p.filter(c => c.id !== id))
    } catch (e) { console.error('Delete case failed:', e) }
  }
  const archiveCase = async (id, archived) => {
    try {
      await invoke('library_archive_case', { caseId: id, archived })
      setCases(p => p.map(c => c.id === id ? {...c, archived} : c))
    } catch (e) { console.error('Archive case failed:', e) }
  }
  const renameCase = async (id) => {
    if (!editName.trim()) return
    try {
      await invoke('library_rename_case', { caseId: id, name: editName.trim() })
      setCases(p => p.map(c => c.id === id ? {...c, name: editName.trim()} : c))
      setEditingCase(null)
    } catch (e) { console.error('Rename case failed:', e) }
  }
  const deleteSession = async (caseId, sessionId) => {
    try {
      await invoke('library_delete_session', { caseId, sessionId })
      setCases(p => p.map(c => c.id === caseId ? {...c, sessions: c.sessions.filter(s => s.id !== sessionId)} : c))
    } catch (e) { console.error('Delete session failed:', e) }
  }

  const handleImportDone = () => {
    setImportModal(false)
    invoke('library_get').then(setCases).catch(() => {})
  }

  return (
    <div className="flex-1 overflow-y-auto flex flex-col min-h-0">
      <div className="flex items-center gap-2.5 px-6 py-3.5 border-b border-border bg-[hsl(var(--surface))] shrink-0">
        <div className="flex-1 max-w-[400px] relative flex items-center">
          <Search size={14} className="absolute left-2.5 text-[hsl(var(--sub))]" />
          <Input
            className="pl-8 pr-8"
            placeholder="Search cases, participants…"
            value={search}
            onChange={e => setSearch(e.target.value)}
          />
          {search && (
            <button
              className="absolute right-2 text-sm text-[hsl(var(--sub))] hover:text-foreground transition-colors"
              onClick={() => setSearch('')}
            >
              <X size={14} />
            </button>
          )}
        </div>
        <Button
          variant="ghost"
          className={cn(showArchived && 'bg-card text-foreground')}
          onClick={() => setShowArchived(p => !p)}
        >
          {showArchived ? 'Hide archived' : 'Show archived'}
        </Button>
        <Button size="sm" onClick={() => setImportModal(true)}>
          <Download size={12} className="shrink-0" />
          Import Audio
        </Button>
        <Button size="sm" onClick={detectSoftware} disabled={scanningCat}>
          {scanningCat ? 'Scanning…' : 'Detect Software'}
        </Button>
      </div>

      {/* CAT Software Detection Results */}
      {catSoftware !== null && (
        <div className="px-6 py-3 border-b border-border">
          {catSoftware.length === 0 ? (
            <p className="text-xs text-[hsl(var(--sub))]">No court reporting software found on this machine.</p>
          ) : (
            <>
              <div className="flex flex-wrap gap-2">
                {catSoftware.map((sw, i) => (
                  <button
                    key={i}
                    className="flex items-center gap-2 px-3 py-1.5 bg-secondary rounded-md text-xs hover:bg-secondary/80 transition-colors"
                    onClick={async () => {
                      const jobs = await invoke('scan_cat_jobs_cmd', { path: sw.path })
                      setCatJobs(jobs)
                    }}
                  >
                    <span className="font-semibold text-foreground">{sw.name}</span>
                    <span className="text-[hsl(var(--sub))]">{sw.jobCount} file{sw.jobCount !== 1 ? 's' : ''}</span>
                  </button>
                ))}
              </div>
              {catJobs.length > 0 && (
                <div className="mt-2 flex flex-col gap-0.5">
                  <span className="font-mono text-[9.5px] uppercase tracking-wider text-[hsl(var(--sub))] mb-1">Available for import:</span>
                  {catJobs.slice(0, 20).map((job, i) => (
                    <div
                      key={i}
                      className="flex items-center gap-3 px-3 py-2 rounded-md cursor-pointer transition-colors hover:bg-secondary/50 text-xs"
                      onClick={() => { if (job.files.length) onReexport(job.files[0].path, job.name) }}
                    >
                      <span className="font-semibold text-foreground flex-1 min-w-0 truncate">{job.name}</span>
                      <span className="text-[hsl(var(--sub))] shrink-0">{job.software}</span>
                      <span className="text-[hsl(var(--sub))] shrink-0">{job.files.length} file{job.files.length !== 1 ? 's' : ''}</span>
                      <span className="text-[hsl(var(--sub))] shrink-0 font-mono text-[10px]">{job.dateModified}</span>
                    </div>
                  ))}
                </div>
              )}
            </>
          )}
        </div>
      )}

      {filtered.length === 0 && !importModal && (
        <div className="flex flex-col items-center justify-center flex-1 gap-2.5 p-16 text-center">
          <Briefcase size={48} className="text-[hsl(var(--sub))] opacity-40" />
          <p className="text-sm font-semibold text-foreground">
            {search ? 'No matches' : cases.length === 0 ? 'No cases yet' : 'No cases to show'}
          </p>
          <p className="text-xs text-[hsl(var(--sub))]">
            {search ? 'Try a different search' : 'Convert a recording or use Import Audio to add files directly'}
          </p>
        </div>
      )}

      <div className="flex flex-col gap-1.5 px-6 py-4 flex-1">
        {filtered.map(c => (
          <Card key={c.id} className={cn(c.archived && 'opacity-60')}>
            <div
              className="flex items-center gap-2.5 px-4 py-3 cursor-pointer transition-colors hover:bg-secondary/50"
              onClick={() => toggleCase(c.id)}
            >
              {expandedCases[c.id]
                ? <ChevronDown size={12} className="text-[hsl(var(--sub))] shrink-0" />
                : <ChevronRight size={12} className="text-[hsl(var(--sub))] shrink-0" />}
              <div className="flex flex-col flex-1 min-w-0">
                {editingCase === c.id ? (
                  <div className="flex items-center gap-2" onClick={e => e.stopPropagation()}>
                    <Input
                      className="h-7 text-sm"
                      value={editName}
                      autoFocus
                      onChange={e => setEditName(e.target.value)}
                      onKeyDown={e => { if (e.key === 'Enter') renameCase(c.id); if (e.key === 'Escape') setEditingCase(null) }}
                    />
                    <Button size="sm" onClick={() => renameCase(c.id)}>Save</Button>
                    <Button variant="ghost" size="sm" onClick={() => setEditingCase(null)}>Cancel</Button>
                  </div>
                ) : (
                  <span className="text-sm font-semibold text-foreground font-serif">{c.name}</span>
                )}
                <span className="text-[11px] text-[hsl(var(--sub))] flex items-center gap-2">
                  {c.sessions.length} session{c.sessions.length !== 1 ? 's' : ''} · {new Date(c.createdAt).toLocaleDateString()}
                  {c.archived && <Badge variant="warning">archived</Badge>}
                </span>
              </div>
              <div className="flex items-center gap-1 shrink-0" onClick={e => e.stopPropagation()}>
                <Button
                  variant="ghost"
                  size="icon"
                  className="h-7 w-7"
                  title="Rename"
                  aria-label="Rename case"
                  onClick={() => { setEditingCase(c.id); setEditName(c.name) }}
                >
                  <Pencil size={12} />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  className="h-7 w-7"
                  title={c.archived ? 'Unarchive' : 'Archive'}
                  aria-label={c.archived ? 'Unarchive case' : 'Archive case'}
                  onClick={() => archiveCase(c.id, !c.archived)}
                >
                  {c.archived ? <RotateCcw size={12} /> : <Archive size={12} />}
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  className="h-7 w-7 hover:text-destructive"
                  title="Delete"
                  aria-label="Delete case"
                  onClick={() => deleteCase(c.id)}
                >
                  <X size={12} />
                </Button>
              </div>
            </div>

            {expandedCases[c.id] && (
              <div className="border-t border-border/60">
                {c.sessions.map(s => (
                  <div key={s.id} className="px-4 py-3 border-b border-border/40 last:border-b-0">
                    <div className="flex items-center gap-2.5 mb-2">
                      <Badge variant="active">{s.date}</Badge>
                      <span className="text-[11px] text-[hsl(var(--text2))] truncate" title={s.sourceFile}>
                        {s.sourceName}
                      </span>
                      <div className="flex items-center gap-1 ml-auto shrink-0">
                        <Button
                          variant="ghost"
                          size="sm"
                          className="h-6 text-[10px]"
                          title="Re-export source"
                          onClick={() => onReexport(s.sourceFile, c.name)}
                        >
                          <RotateCcw size={10} /> Re-export
                        </Button>
                        <Button
                          variant="ghost"
                          size="icon"
                          className="h-6 w-6 hover:text-destructive"
                          title="Remove session"
                          onClick={() => deleteSession(c.id, s.id)}
                        >
                          <X size={10} />
                        </Button>
                      </div>
                    </div>
                    <div className="flex flex-col gap-2">
                      {s.participants.map((p, pi) => (
                        <div key={pi} className="flex flex-col gap-1">
                          <div className="flex items-center gap-2">
                            <span className="w-2 h-2 rounded-full shrink-0" style={{ background: CH_COLORS[pi % 4] }} />
                            <span className="text-[11px] font-semibold text-[hsl(var(--text2))]">{p.label}</span>
                          </div>
                          <div className="flex flex-col gap-1 ml-4">
                            {p.files.map((f, fi) => <LibraryFile key={fi} file={f} />)}
                          </div>
                        </div>
                      ))}
                    </div>
                  </div>
                ))}
              </div>
            )}
          </Card>
        ))}
      </div>

      {importModal && (
        <ImportModal
          defaultLabels={labels}
          existingCases={cases.filter(c => !c.archived).map(c => c.name)}
          onDone={handleImportDone}
          onClose={() => setImportModal(false)}
        />
      )}
    </div>
  )
}
