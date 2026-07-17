import { useState, useMemo } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { Search, Download, ChevronDown, ChevronRight, Pencil, Archive, RotateCcw, X, Briefcase, Loader2, FolderSearch } from 'lucide-react'
import { cn } from '../../lib/utils'
import { useSpeakerColors, speakerColorAt } from '../../lib/speakerColors'
import { usePreferencesContext } from '../../hooks/PreferencesContext'
import { Button } from '../ui/button'
import { Input } from '../ui/input'
import { Badge } from '../ui/badge'
import { Card, CardHeader, CardTitle, CardContent } from '../ui/card'
import { Segmented } from '../ui/segmented'
import { ConfirmDialog } from '../ui/confirm-dialog'
import LibraryFile from './LibraryFile'
import ImportModal from './ImportModal'

const CAT_JOB_LIMIT = 20

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
  const [confirm, setConfirm]               = useState(null) // { title, description, onConfirm }
  const speakerColors = useSpeakerColors()

  const detectSoftware = async () => {
    setScanningCat(true)
    try {
      const sw = await invoke('detect_cat_software_cmd', { maxDepth: maxScanDepth })
      setCatSoftware(sw)
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

  const toggleCase = (id) => setExpandedCases(p => ({ ...p, [id]: !p[id] }))

  const deleteCase = (id, name) => setConfirm({
    title: 'Delete this case?',
    description: `“${name}” and all its session records will be removed from the library. Files on disk are not deleted.`,
    confirmLabel: 'Delete case',
    onConfirm: async () => {
      try {
        await invoke('library_delete_case', { caseId: id })
        setCases(p => p.filter(c => c.id !== id))
      } catch (e) { console.error('Delete case failed:', e) }
    },
  })
  const archiveCase = async (id, archived) => {
    try {
      await invoke('library_archive_case', { caseId: id, archived })
      setCases(p => p.map(c => c.id === id ? { ...c, archived } : c))
    } catch (e) { console.error('Archive case failed:', e) }
  }
  const renameCase = async (id) => {
    if (!editName.trim()) return
    try {
      await invoke('library_rename_case', { caseId: id, name: editName.trim() })
      setCases(p => p.map(c => c.id === id ? { ...c, name: editName.trim() } : c))
      setEditingCase(null)
    } catch (e) { console.error('Rename case failed:', e) }
  }
  const deleteSession = (caseId, sessionId, sourceName) => setConfirm({
    title: 'Remove this session?',
    description: `The session record for “${sourceName}” will be removed from the case. Files on disk are not deleted.`,
    confirmLabel: 'Remove session',
    onConfirm: async () => {
      try {
        await invoke('library_delete_session', { caseId, sessionId })
        setCases(p => p.map(c => c.id === caseId ? { ...c, sessions: c.sessions.filter(s => s.id !== sessionId) } : c))
      } catch (e) { console.error('Delete session failed:', e) }
    },
  })

  const handleImportDone = () => {
    setImportModal(false)
    invoke('library_get').then(setCases).catch(() => {})
  }

  const activeCount = cases.filter(c => !c.archived).length
  const archivedCount = cases.filter(c => c.archived).length

  return (
    <div className="flex-1 overflow-y-auto overflow-x-hidden">
      {/* Sticky toolbar — search stays put while the list scrolls */}
      <div className="sticky top-0 z-10 flex flex-wrap items-center gap-2.5 px-5 md:px-8 py-3 border-b border-border bg-[hsl(var(--surface))]">
        <div className="flex-1 min-w-[220px] max-w-[420px] relative flex items-center">
          <Search size={14} className="absolute left-2.5 text-[hsl(var(--sub))]" />
          <Input
            className="pl-8 pr-8"
            placeholder="Search cases, participants…"
            value={search}
            onChange={e => setSearch(e.target.value)}
          />
          {search && (
            <button
              className="absolute right-2 text-[hsl(var(--sub))] hover:text-foreground transition-colors"
              aria-label="Clear search"
              onClick={() => setSearch('')}
            >
              <X size={14} />
            </button>
          )}
        </div>
        <Segmented
          size="sm"
          aria-label="Show active or archived cases"
          value={showArchived ? 'archived' : 'active'}
          onChange={v => setShowArchived(v === 'archived')}
          options={[
            { value: 'active', label: `Active${activeCount ? ` ${activeCount}` : ''}` },
            { value: 'archived', label: `Archived${archivedCount ? ` ${archivedCount}` : ''}` },
          ]}
        />
        <div className="flex-1" />
        <Button size="sm" onClick={() => setImportModal(true)}>
          <Download size={12} className="shrink-0" /> Import audio
        </Button>
        <Button variant="ghost" size="sm" onClick={detectSoftware} disabled={scanningCat}>
          {scanningCat ? <><Loader2 className="h-3.5 w-3.5 animate-spin" /> Scanning…</> : <><FolderSearch size={13} /> Find court software</>}
        </Button>
      </div>

      <div className="w-full max-w-[1100px] mx-auto px-5 md:px-8 py-5 flex flex-col gap-3.5">

        {/* Court reporting software — a proper dismissible Card */}
        {catSoftware !== null && (
          <Card>
            <CardHeader>
              <CardTitle>Court reporting software</CardTitle>
              <button
                className="text-[hsl(var(--sub))] hover:text-foreground transition-colors rounded p-0.5 focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring"
                aria-label="Dismiss court-software results"
                onClick={() => { setCatSoftware(null); setCatJobs([]) }}
              >
                <X size={14} />
              </button>
            </CardHeader>
            <CardContent className="p-4 flex flex-col gap-3">
              {catSoftware.length === 0 ? (
                <p className="text-[12px] text-[hsl(var(--sub))]">No court reporting software found on this machine.</p>
              ) : (
                <>
                  <div className="flex flex-wrap gap-1.5">
                    {catSoftware.map((sw, i) => (
                      <button
                        key={i}
                        className="flex items-center gap-2 px-2.5 py-1.5 bg-secondary rounded-md text-[12px] hover:bg-secondary/70 transition-colors focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring"
                        onClick={async () => {
                          try { setCatJobs(await invoke('scan_cat_jobs_cmd', { path: sw.path })) } catch { setCatJobs([]) }
                        }}
                      >
                        <span className="font-medium text-foreground">{sw.name}</span>
                        <Badge variant="default">{sw.jobCount} file{sw.jobCount !== 1 ? 's' : ''}</Badge>
                      </button>
                    ))}
                  </div>
                  {catJobs.length > 0 && (
                    <div className="flex flex-col">
                      <div className="flex items-center justify-between px-1 pb-1.5">
                        <span className="font-mono text-[9px] font-medium tracking-[1.2px] uppercase text-[hsl(var(--sub))]">Available to import</span>
                        <span className="font-mono text-[10px] text-[hsl(var(--sub))]">
                          {catJobs.length > CAT_JOB_LIMIT ? `showing ${CAT_JOB_LIMIT} of ${catJobs.length}` : `${catJobs.length} found`}
                        </span>
                      </div>
                      <div className="flex flex-col">
                        {catJobs.slice(0, CAT_JOB_LIMIT).map((job, i) => (
                          <button
                            key={i}
                            className="flex items-center gap-3 px-2 py-2 rounded-md transition-colors hover:bg-secondary/50 text-[12px] text-left focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring"
                            onClick={() => { if (job.files.length) onReexport(job.files[0].path, job.name) }}
                          >
                            <span className="font-medium text-foreground flex-1 min-w-0 truncate">{job.name}</span>
                            <span className="text-[hsl(var(--sub))] shrink-0">{job.software}</span>
                            <Badge variant="default">{job.files.length} file{job.files.length !== 1 ? 's' : ''}</Badge>
                            <span className="text-[hsl(var(--sub))] shrink-0 font-mono text-[10px]">{job.dateModified}</span>
                          </button>
                        ))}
                      </div>
                    </div>
                  )}
                </>
              )}
            </CardContent>
          </Card>
        )}

        {/* Cases */}
        {filtered.length === 0 ? (
          <div className="flex flex-col items-center justify-center gap-2.5 py-20 text-center">
            <Briefcase size={44} className="text-[hsl(var(--sub))] opacity-40" />
            <p className="text-[13px] font-semibold text-foreground">
              {search ? 'No matches' : showArchived ? 'No archived cases' : cases.length === 0 ? 'No cases yet' : 'No active cases'}
            </p>
            <p className="text-[12px] text-[hsl(var(--sub))] max-w-[340px]">
              {search
                ? 'Try a different search term, or clear the search to see everything.'
                : 'Convert a recording on the Convert tab, or use Import audio to add existing files — each one is auto-filed here by case and participant.'}
            </p>
          </div>
        ) : (
          <Card>
            <CardHeader>
              <CardTitle>{showArchived ? 'Archived cases' : 'Cases'}</CardTitle>
              <span className="font-mono text-[10px] text-[hsl(var(--sub))]">{filtered.length}</span>
            </CardHeader>
            <CardContent>
              {filtered.map((c, ci) => {
                const expanded = !!expandedCases[c.id]
                return (
                  <div key={c.id} className={cn(ci > 0 && 'border-t border-border/50', c.archived && 'opacity-70')}>
                    {/* Case row header — a real button for keyboard access */}
                    <div className="flex items-center gap-2.5 px-4 py-3">
                      <button
                        className="flex items-center gap-2.5 flex-1 min-w-0 text-left focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring rounded"
                        aria-expanded={expanded}
                        onClick={() => toggleCase(c.id)}
                      >
                        {expanded
                          ? <ChevronDown size={13} className="text-[hsl(var(--sub))] shrink-0" />
                          : <ChevronRight size={13} className="text-[hsl(var(--sub))] shrink-0" />}
                        <div className="flex flex-col flex-1 min-w-0">
                          {editingCase === c.id ? (
                            <div className="flex items-center gap-2" onClick={e => { e.stopPropagation(); e.preventDefault() }}>
                              <Input
                                className="h-7 text-[13px]"
                                value={editName}
                                autoFocus
                                onChange={e => setEditName(e.target.value)}
                                onKeyDown={e => { if (e.key === 'Enter') renameCase(c.id); if (e.key === 'Escape') setEditingCase(null) }}
                              />
                              <Button size="sm" onClick={() => renameCase(c.id)}>Save</Button>
                              <Button variant="ghost" size="sm" onClick={() => setEditingCase(null)}>Cancel</Button>
                            </div>
                          ) : (
                            <span className="text-[14px] font-semibold text-foreground font-serif truncate">{c.name}</span>
                          )}
                          <span className="text-[11px] text-[hsl(var(--sub))] flex items-center gap-2">
                            {c.sessions.length} session{c.sessions.length !== 1 ? 's' : ''} · {new Date(c.createdAt).toLocaleDateString()}
                            {c.archived && <Badge variant="warning">archived</Badge>}
                          </span>
                        </div>
                      </button>
                      <div className="flex items-center gap-0.5 shrink-0">
                        <Button variant="ghost" size="icon" className="h-7 w-7" title="Rename" aria-label={`Rename ${c.name}`}
                          onClick={() => { setEditingCase(c.id); setEditName(c.name) }}>
                          <Pencil size={12} />
                        </Button>
                        <Button variant="ghost" size="icon" className="h-7 w-7" title={c.archived ? 'Unarchive' : 'Archive'}
                          aria-label={c.archived ? `Unarchive ${c.name}` : `Archive ${c.name}`}
                          onClick={() => archiveCase(c.id, !c.archived)}>
                          {c.archived ? <RotateCcw size={12} /> : <Archive size={12} />}
                        </Button>
                        <Button variant="ghost" size="icon" className="h-7 w-7 hover:text-destructive" title="Delete"
                          aria-label={`Delete ${c.name}`} onClick={() => deleteCase(c.id, c.name)}>
                          <X size={12} />
                        </Button>
                      </div>
                    </div>

                    {expanded && (
                      <div className="bg-secondary/30 border-t border-border/50">
                        {c.sessions.map(s => (
                          <div key={s.id} className="px-4 py-3 border-b border-border/40 last:border-b-0">
                            <div className="flex items-center gap-2.5 mb-2">
                              <Badge variant="active">{s.date}</Badge>
                              <span className="text-[11px] text-[hsl(var(--text2))] truncate" title={s.sourceFile}>{s.sourceName}</span>
                              <div className="flex items-center gap-1 ml-auto shrink-0">
                                <Button variant="ghost" size="sm" className="h-6 text-[10px]" title="Re-export source"
                                  onClick={() => onReexport(s.sourceFile, c.name)}>
                                  <RotateCcw size={10} /> Re-export
                                </Button>
                                <Button variant="ghost" size="icon" className="h-6 w-6 hover:text-destructive" title="Remove session"
                                  aria-label="Remove session" onClick={() => deleteSession(c.id, s.id, s.sourceName)}>
                                  <X size={10} />
                                </Button>
                              </div>
                            </div>
                            <div className="flex flex-col gap-2">
                              {s.participants.map((p, pi) => (
                                <div key={pi} className="flex flex-col gap-1">
                                  <div className="flex items-center gap-2">
                                    <span className="w-2 h-2 rounded-full shrink-0" style={{ background: speakerColorAt(speakerColors, pi) }} />
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
                  </div>
                )
              })}
            </CardContent>
          </Card>
        )}
      </div>

      {importModal && (
        <ImportModal
          defaultLabels={labels}
          existingCases={cases.filter(c => !c.archived).map(c => c.name)}
          onDone={handleImportDone}
          onClose={() => setImportModal(false)}
        />
      )}

      <ConfirmDialog
        open={!!confirm}
        onOpenChange={(o) => { if (!o) setConfirm(null) }}
        title={confirm?.title}
        description={confirm?.description}
        confirmLabel={confirm?.confirmLabel}
        onConfirm={() => confirm?.onConfirm?.()}
      />
    </div>
  )
}
