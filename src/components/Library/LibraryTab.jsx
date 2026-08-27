import { useEffect, useMemo, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import {
  Search,
  Download,
  ChevronDown,
  ChevronRight,
  Pencil,
  Archive,
  RotateCcw,
  X,
  Briefcase,
  Loader2,
  FolderSearch,
} from 'lucide-react'
import { cn } from '../../lib/utils'
import { useSpeakerColors, speakerColorAt } from '../../lib/speakerColors'
import { usePreferencesContext } from '../../hooks/PreferencesContext'
import { Button } from '../ui/button'
import { Input } from '../ui/input'
import { Label } from '../ui/label'
import { Badge } from '../ui/badge'
import { Card, CardHeader, CardTitle, CardContent } from '../ui/card'
import { Segmented } from '../ui/segmented'
import { ConfirmDialog } from '../ui/confirm-dialog'
import WorkspaceHeader from '../common/WorkspaceHeader'
import LibraryFile from './LibraryFile'
import ImportModal from './ImportModal'
import { catJobPaths } from '../../lib/queue'

const CAT_JOB_PAGE_SIZE = 20

export default function LibraryTab({
  cases,
  setCases,
  search,
  setSearch,
  labels,
  onReexport,
  conversionLocked = false,
  readOnly = false,
  onStorageError,
  onReload,
}) {
  const { maxScanDepth } = usePreferencesContext()
  const [showArchived, setShowArchived] = useState(false)
  const [expandedCases, setExpandedCases] = useState({})
  const [editingCase, setEditingCase] = useState(null)
  const [editName, setEditName] = useState('')
  const [importModal, setImportModal] = useState(false)
  const [catSoftware, setCatSoftware] = useState(null)
  const [catJobs, setCatJobs] = useState([])
  const [visibleCatJobCount, setVisibleCatJobCount] = useState(CAT_JOB_PAGE_SIZE)
  const [scanningCat, setScanningCat] = useState(false)
  const [confirm, setConfirm] = useState(null) // { title, description, onConfirm }
  const [operationWarning, setOperationWarning] = useState('')
  const renameButtonRefs = useRef(new Map())
  const returnFocusCaseRef = useRef(null)
  const speakerColors = useSpeakerColors()

  useEffect(() => {
    if (editingCase !== null || returnFocusCaseRef.current === null) return
    const caseId = returnFocusCaseRef.current
    returnFocusCaseRef.current = null
    renameButtonRefs.current.get(caseId)?.focus()
  }, [editingCase])

  const finishRenaming = id => {
    returnFocusCaseRef.current = id
    setEditingCase(null)
  }

  const detectSoftware = async () => {
    setScanningCat(true)
    setCatJobs([])
    setVisibleCatJobCount(CAT_JOB_PAGE_SIZE)
    setOperationWarning('')
    try {
      const sw = await invoke('detect_cat_software_cmd', { maxDepth: maxScanDepth })
      setCatSoftware(sw)
      if (sw.length > 0) {
        const jobs = await invoke('scan_cat_jobs_cmd', { path: sw[0].path, maxDepth: maxScanDepth })
        setCatJobs(jobs)
      }
    } catch (e) {
      console.error('CAT detection failed:', e)
      setCatSoftware([])
      setOperationWarning({
        summary: 'Court-software detection could not finish. Try again, or import audio manually.',
        detail: String(e),
      })
    }
    setScanningCat(false)
  }

  const filtered = useMemo(
    () =>
      cases
        .filter(c => (showArchived ? c.archived : !c.archived))
        .filter(c => {
          if (!search.trim()) return true
          const q = search.toLowerCase()
          return (
            c.name.toLowerCase().includes(q) ||
            c.sessions.some(
              s =>
                s.sourceName?.toLowerCase().includes(q) || s.participants.some(p => p.label.toLowerCase().includes(q)),
            )
          )
        }),
    [cases, showArchived, search],
  )

  const toggleCase = id => setExpandedCases(p => ({ ...p, [id]: !p[id] }))

  const deleteCase = (id, name) => {
    if (readOnly) return
    setConfirm({
      title: 'Delete this case?',
      description: `“${name}” and all its session records will be removed from the library. Files on disk are not deleted.`,
      confirmLabel: 'Delete case',
      onConfirm: async () => {
        try {
          const outcome = await invoke('library_delete_case', { caseId: id })
          if (outcome?.changed !== false) setCases(p => p.filter(c => c.id !== id))
          setOperationWarning(outcome?.warning || '')
        } catch (e) {
          console.error('Delete case failed:', e)
          onStorageError?.(e)
        }
      },
    })
  }
  const archiveCase = async (id, archived) => {
    if (readOnly) return
    try {
      await invoke('library_archive_case', { caseId: id, archived })
      setCases(p => p.map(c => (c.id === id ? { ...c, archived } : c)))
    } catch (e) {
      console.error('Archive case failed:', e)
      onStorageError?.(e)
    }
  }
  const renameCase = async id => {
    if (readOnly || !editName.trim()) return
    try {
      await invoke('library_rename_case', { caseId: id, name: editName.trim() })
      setCases(p => p.map(c => (c.id === id ? { ...c, name: editName.trim() } : c)))
      finishRenaming(id)
    } catch (e) {
      console.error('Rename case failed:', e)
      onStorageError?.(e)
    }
  }
  const deleteSession = (caseId, sessionId, sourceName) => {
    if (readOnly) return
    setConfirm({
      title: 'Remove this session?',
      description: `The session record for “${sourceName}” will be removed from the case. Files on disk are not deleted.`,
      confirmLabel: 'Remove session',
      onConfirm: async () => {
        try {
          const outcome = await invoke('library_delete_session', { caseId, sessionId })
          if (outcome?.changed !== false) {
            setCases(p =>
              p.map(c => (c.id === caseId ? { ...c, sessions: c.sessions.filter(s => s.id !== sessionId) } : c)),
            )
          }
          setOperationWarning(outcome?.warning || '')
        } catch (e) {
          console.error('Delete session failed:', e)
          onStorageError?.(e)
        }
      },
    })
  }

  const handleImportDone = warning => {
    setImportModal(false)
    setOperationWarning(warning || '')
    onReload?.()
  }

  const activeCount = cases.filter(c => !c.archived).length
  const archivedCount = cases.filter(c => c.archived).length
  const visibleCaseLabel = showArchived ? 'archived' : 'active'
  const searchStatus = readOnly
    ? 'Library data is unavailable.'
    : `${filtered.length} ${visibleCaseLabel} case${filtered.length === 1 ? '' : 's'}${
        search.trim() ? ` ${filtered.length === 1 ? 'matches' : 'match'} the search.` : '.'
      }`
  const catStatus = scanningCat
    ? 'Scanning for court software and jobs.'
    : catSoftware === null
      ? ''
      : catSoftware.length === 0
        ? 'No court-software installations were found.'
        : `Found ${catSoftware.length} court-software installation${catSoftware.length === 1 ? '' : 's'} and ${catJobs.length} job${catJobs.length === 1 ? '' : 's'}.`

  return (
    <div className="flex-1 overflow-y-auto overflow-x-hidden">
      <div className="w-full max-w-[1100px] mx-auto px-4 pt-5 pb-4 sm:px-6 lg:px-8">
        <WorkspaceHeader
          eyebrow="Case organization"
          title="Find case audio"
          description="Keep converted sessions grouped by case and participant, or locate recordings from installed court software."
          status="Library records stay on this computer"
        />
      </div>
      {/* Sticky toolbar — search stays put while the list scrolls */}
      <div className="library-toolbar sticky top-0 z-10 flex flex-wrap items-center gap-2.5 px-5 md:px-8 py-3 border-b border-border bg-[hsl(var(--surface))]">
        <div className="library-search flex-1 min-w-[220px] max-w-[420px] relative flex items-center">
          <Label htmlFor="library-search" className="sr-only">
            Search Library cases and participants
          </Label>
          <Search size={14} aria-hidden="true" className="absolute left-2.5 text-[hsl(var(--sub))]" />
          <Input
            id="library-search"
            type="search"
            aria-controls="library-case-list"
            aria-describedby="library-search-status"
            className="pl-8 pr-8"
            placeholder="Search cases, participants…"
            value={search}
            onChange={e => setSearch(e.target.value)}
          />
          {search && (
            <button
              type="button"
              className="absolute right-2 rounded-sm p-1 text-[hsl(var(--sub))] hover:text-foreground transition-colors focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1"
              aria-label="Clear search"
              onClick={() => setSearch('')}
            >
              <X size={14} aria-hidden="true" />
            </button>
          )}
        </div>
        <Segmented
          size="sm"
          className="library-filter"
          aria-label="Show active or archived cases"
          aria-controls="library-case-list"
          value={showArchived ? 'archived' : 'active'}
          onChange={v => setShowArchived(v === 'archived')}
          options={[
            { value: 'active', label: `Active${activeCount ? ` ${activeCount}` : ''}` },
            { value: 'archived', label: `Archived${archivedCount ? ` ${archivedCount}` : ''}` },
          ]}
        />
        <div className="library-toolbar-spacer flex-1" />
        <Button
          type="button"
          size="sm"
          className="library-toolbar-action"
          onClick={() => setImportModal(true)}
          disabled={readOnly}
          title={readOnly ? 'Library storage is read-only until repaired' : undefined}
        >
          <Download size={12} aria-hidden="true" className="shrink-0" /> Import audio
        </Button>
        <Button
          type="button"
          variant="ghost"
          size="sm"
          className="library-toolbar-action"
          onClick={detectSoftware}
          disabled={scanningCat}
          aria-describedby="library-cat-status"
        >
          {scanningCat ? (
            <>
              <Loader2 className="h-3.5 w-3.5 animate-spin" aria-hidden="true" /> Scanning…
            </>
          ) : (
            <>
              <FolderSearch size={13} aria-hidden="true" /> Find court software
            </>
          )}
        </Button>
        <span
          id="library-cat-status"
          role="status"
          aria-label="Court software scan status"
          aria-live="polite"
          aria-atomic="true"
          className="sr-only"
        >
          {catStatus}
        </span>
      </div>

      <div className="library-content w-full max-w-[1100px] mx-auto px-4 sm:px-6 lg:px-8 py-5 flex flex-col gap-3.5">
        <span id="library-search-status" role="status" aria-live="polite" aria-atomic="true" className="sr-only">
          {searchStatus}
        </span>
        {operationWarning && (
          <div
            role="alert"
            className="flex items-start gap-2 rounded-md border border-warning/30 bg-warning/10 px-3 py-2 text-xs text-foreground"
          >
            <div className="flex-1">
              <span>{typeof operationWarning === 'string' ? operationWarning : operationWarning.summary}</span>
              {typeof operationWarning !== 'string' && operationWarning.detail && (
                <details className="mt-1.5">
                  <summary className="w-fit cursor-pointer font-semibold text-[hsl(var(--text2))] focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1">
                    Technical details
                  </summary>
                  <p className="mt-1 whitespace-pre-wrap break-words text-[10px] text-[hsl(var(--sub))]">
                    {operationWarning.detail}
                  </p>
                </details>
              )}
            </div>
            <button
              type="button"
              aria-label="Dismiss Library warning"
              className="rounded-sm p-1 text-[hsl(var(--sub))] hover:text-foreground focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1"
              onClick={() => setOperationWarning('')}
            >
              <X size={13} aria-hidden="true" />
            </button>
          </div>
        )}

        {/* Court reporting software — a proper dismissible Card */}
        {catSoftware !== null && (
          <Card>
            <CardHeader>
              <CardTitle>Court reporting software</CardTitle>
              <button
                type="button"
                className="text-[hsl(var(--sub))] hover:text-foreground transition-colors rounded p-1 focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1"
                aria-label="Dismiss court-software results"
                onClick={() => {
                  setCatSoftware(null)
                  setCatJobs([])
                  setVisibleCatJobCount(CAT_JOB_PAGE_SIZE)
                }}
              >
                <X size={14} aria-hidden="true" />
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
                        type="button"
                        className="flex items-center gap-2 px-2.5 py-1.5 bg-secondary rounded-md text-[12px] hover:bg-secondary/70 transition-colors focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1"
                        onClick={async () => {
                          setCatJobs([])
                          setVisibleCatJobCount(CAT_JOB_PAGE_SIZE)
                          try {
                            setCatJobs(await invoke('scan_cat_jobs_cmd', { path: sw.path, maxDepth: maxScanDepth }))
                          } catch (error) {
                            setCatJobs([])
                            setOperationWarning({
                              summary:
                                'Court-job scanning could not finish. Choose the software again or import audio manually.',
                              detail: String(error),
                            })
                          }
                        }}
                      >
                        <span className="font-medium text-foreground">{sw.name}</span>
                        <Badge variant="default">
                          {sw.jobCount} file{sw.jobCount !== 1 ? 's' : ''}
                        </Badge>
                      </button>
                    ))}
                  </div>
                  {catJobs.length > 0 && (
                    <div className="flex flex-col">
                      <div className="flex items-center justify-between px-1 pb-1.5">
                        <span className="font-mono text-[9px] font-medium tracking-[1.2px] uppercase text-[hsl(var(--sub))]">
                          Available to import
                        </span>
                        <span className="font-mono text-[10px] text-[hsl(var(--sub))]">
                          {visibleCatJobCount < catJobs.length
                            ? `Showing ${visibleCatJobCount} of ${catJobs.length}`
                            : `${catJobs.length} found`}
                        </span>
                      </div>
                      <div className="flex flex-col">
                        {catJobs.slice(0, visibleCatJobCount).map((job, i) => (
                          <button
                            key={`${job.path}:${job.name}:${i}`}
                            type="button"
                            aria-label={`Queue ${job.files.length} file${job.files.length === 1 ? '' : 's'} from ${job.name}`}
                            className="flex items-center gap-3 px-2 py-2 rounded-md transition-colors hover:bg-secondary/50 text-[12px] text-left focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1"
                            disabled={conversionLocked || job.files.length === 0}
                            title={
                              conversionLocked
                                ? 'Wait for the current conversion to finish'
                                : `Queue all ${job.files.length} files`
                            }
                            onClick={() => {
                              if (job.files.length) onReexport(catJobPaths(job), job.name)
                            }}
                          >
                            <span className="font-medium text-foreground flex-1 min-w-0 truncate">{job.name}</span>
                            <span className="text-[hsl(var(--sub))] shrink-0">{job.software}</span>
                            <Badge variant="default">
                              {job.files.length} file{job.files.length !== 1 ? 's' : ''}
                            </Badge>
                            <span className="text-[hsl(var(--sub))] shrink-0 font-mono text-[10px]">
                              {job.dateModified}
                            </span>
                          </button>
                        ))}
                        {visibleCatJobCount < catJobs.length && (
                          <Button
                            type="button"
                            variant="ghost"
                            size="sm"
                            className="mt-1 self-center"
                            onClick={() =>
                              setVisibleCatJobCount(count => Math.min(count + CAT_JOB_PAGE_SIZE, catJobs.length))
                            }
                          >
                            Show {Math.min(CAT_JOB_PAGE_SIZE, catJobs.length - visibleCatJobCount)} more
                          </Button>
                        )}
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
          <div
            id="library-case-list"
            aria-label={`${visibleCaseLabel} Library cases`}
            className="flex flex-col items-center justify-center gap-2.5 py-14 text-center"
          >
            <Briefcase size={44} aria-hidden="true" className="text-[hsl(var(--sub))] opacity-40" />
            <p className="text-[13px] font-semibold text-foreground">
              {readOnly
                ? 'Library unavailable'
                : search
                  ? 'No matches'
                  : showArchived
                    ? 'No archived cases'
                    : cases.length === 0
                      ? 'No cases yet'
                      : 'No active cases'}
            </p>
            <p className="text-[12px] text-[hsl(var(--sub))] max-w-[340px]">
              {readOnly
                ? 'Repair the library data file and restart DepoAudio before making library changes.'
                : search
                  ? 'Try a different search term, or clear the search to see everything.'
                  : 'Convert a recording on the Convert tab, or use Import audio to add existing files — each one is auto-filed here by case and participant.'}
            </p>
            {readOnly ? (
              <Button type="button" variant="outline" className="mt-2" onClick={onReload}>
                Retry Library
              </Button>
            ) : (
              <div className="mt-2 flex flex-wrap items-center justify-center gap-2">
                <Button type="button" onClick={() => setImportModal(true)}>
                  <Download size={12} aria-hidden="true" /> Import audio
                </Button>
                <Button type="button" variant="ghost" onClick={detectSoftware} disabled={scanningCat}>
                  <FolderSearch size={13} aria-hidden="true" /> Scan installed court software
                </Button>
              </div>
            )}
          </div>
        ) : (
          <Card id="library-case-list" aria-label={`${visibleCaseLabel} Library cases`}>
            <CardHeader>
              <CardTitle>{showArchived ? 'Archived cases' : 'Cases'}</CardTitle>
              <span className="font-mono text-[10px] text-[hsl(var(--sub))]">{filtered.length}</span>
            </CardHeader>
            <CardContent>
              {filtered.map((c, ci) => {
                const expanded = !!expandedCases[c.id]
                const panelId = `library-case-${ci}-sessions`
                return (
                  <div key={c.id} className={cn(ci > 0 && 'border-t border-border/50', c.archived && 'opacity-70')}>
                    {/* Case row header — a real button for keyboard access */}
                    <div className="library-case-header flex items-center gap-2.5 px-4 py-3">
                      <button
                        type="button"
                        className="library-case-toggle flex items-center gap-2.5 flex-1 min-w-0 text-left focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 rounded"
                        aria-expanded={expanded}
                        aria-controls={panelId}
                        aria-label={`${expanded ? 'Collapse' : 'Expand'} ${c.name}, ${c.sessions.length} session${c.sessions.length === 1 ? '' : 's'}`}
                        onClick={() => toggleCase(c.id)}
                      >
                        {expanded ? (
                          <ChevronDown size={13} aria-hidden="true" className="text-[hsl(var(--sub))] shrink-0" />
                        ) : (
                          <ChevronRight size={13} aria-hidden="true" className="text-[hsl(var(--sub))] shrink-0" />
                        )}
                        <div className="flex flex-col flex-1 min-w-0">
                          <span className="text-[14px] font-semibold text-foreground truncate">{c.name}</span>
                          <span className="text-[11px] text-[hsl(var(--sub))] flex items-center gap-2">
                            {c.sessions.length} session{c.sessions.length !== 1 ? 's' : ''} ·{' '}
                            {new Date(c.createdAt).toLocaleDateString()}
                            {c.archived && <Badge variant="warning">archived</Badge>}
                          </span>
                        </div>
                      </button>
                      <div className="library-case-actions flex items-center gap-0.5 shrink-0">
                        <Button
                          ref={node => {
                            if (node) renameButtonRefs.current.set(c.id, node)
                            else renameButtonRefs.current.delete(c.id)
                          }}
                          type="button"
                          variant="ghost"
                          size="icon"
                          className="h-7 w-7"
                          title="Rename"
                          aria-label={`Rename ${c.name}`}
                          disabled={readOnly || editingCase === c.id}
                          onClick={() => {
                            setEditingCase(c.id)
                            setEditName(c.name)
                          }}
                        >
                          <Pencil size={12} aria-hidden="true" />
                        </Button>
                        <Button
                          type="button"
                          variant="ghost"
                          size="icon"
                          className="h-7 w-7"
                          title={c.archived ? 'Unarchive' : 'Archive'}
                          aria-label={c.archived ? `Unarchive ${c.name}` : `Archive ${c.name}`}
                          disabled={readOnly}
                          onClick={() => archiveCase(c.id, !c.archived)}
                        >
                          {c.archived ? (
                            <RotateCcw size={12} aria-hidden="true" />
                          ) : (
                            <Archive size={12} aria-hidden="true" />
                          )}
                        </Button>
                        <Button
                          type="button"
                          variant="ghost"
                          size="icon"
                          className="h-7 w-7 hover:text-destructive"
                          title="Delete"
                          disabled={readOnly}
                          aria-label={`Delete ${c.name}`}
                          onClick={() => deleteCase(c.id, c.name)}
                        >
                          <X size={12} aria-hidden="true" />
                        </Button>
                      </div>
                      {editingCase === c.id && (
                        <form
                          className="library-case-rename flex min-w-0 items-center gap-2"
                          onSubmit={event => {
                            event.preventDefault()
                            renameCase(c.id)
                          }}
                        >
                          <Label htmlFor={`library-case-${ci}-name`} className="sr-only">
                            Rename {c.name}
                          </Label>
                          <Input
                            id={`library-case-${ci}-name`}
                            className="h-7 min-w-0 text-[13px]"
                            value={editName}
                            autoFocus
                            onChange={event => setEditName(event.target.value)}
                            onKeyDown={event => {
                              if (event.key === 'Escape') {
                                event.preventDefault()
                                finishRenaming(c.id)
                              }
                            }}
                          />
                          <Button type="submit" size="sm" disabled={readOnly || !editName.trim()}>
                            Save
                          </Button>
                          <Button type="button" variant="ghost" size="sm" onClick={() => finishRenaming(c.id)}>
                            Cancel
                          </Button>
                        </form>
                      )}
                    </div>

                    <div id={panelId} hidden={!expanded} className="bg-secondary/30 border-t border-border/50">
                      {expanded &&
                        c.sessions.map(s => (
                          <div key={s.id} className="px-4 py-3 border-b border-border/40 last:border-b-0">
                            <div className="library-session-header flex items-center gap-2.5 mb-2">
                              <Badge variant="active">{s.date}</Badge>
                              <span
                                className="library-session-name text-[11px] text-[hsl(var(--text2))] truncate"
                                title={s.sourceFile}
                              >
                                {s.sourceName}
                              </span>
                              <div className="library-session-actions flex items-center gap-1 ml-auto shrink-0">
                                <Button
                                  type="button"
                                  variant="ghost"
                                  size="sm"
                                  className="h-6 text-[10px]"
                                  title={
                                    conversionLocked ? 'Wait for the current conversion to finish' : 'Re-export source'
                                  }
                                  disabled={conversionLocked}
                                  onClick={() => onReexport(s.sourceFile, c.name)}
                                >
                                  <RotateCcw size={10} aria-hidden="true" /> Re-export
                                </Button>
                                <Button
                                  type="button"
                                  variant="ghost"
                                  size="icon"
                                  className="h-6 w-6 hover:text-destructive"
                                  title="Remove session"
                                  disabled={readOnly}
                                  aria-label={`Remove ${s.sourceName} from ${c.name}`}
                                  onClick={() => deleteSession(c.id, s.id, s.sourceName)}
                                >
                                  <X size={10} aria-hidden="true" />
                                </Button>
                              </div>
                            </div>
                            <div className="flex flex-col gap-2">
                              {s.participants.map((p, pi) => (
                                <div key={pi} className="flex flex-col gap-1">
                                  <div className="flex items-center gap-2">
                                    <span
                                      aria-hidden="true"
                                      className="w-2 h-2 rounded-full shrink-0"
                                      style={{ background: speakerColorAt(speakerColors, pi) }}
                                    />
                                    <span className="text-[11px] font-semibold text-[hsl(var(--text2))]">
                                      {p.label}
                                    </span>
                                  </div>
                                  <div className="library-participant-files flex flex-col gap-1 ml-4">
                                    {p.files.map((f, fi) => (
                                      <LibraryFile key={fi} file={f} />
                                    ))}
                                  </div>
                                </div>
                              ))}
                            </div>
                          </div>
                        ))}
                    </div>
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
          onStorageError={onStorageError}
        />
      )}

      <ConfirmDialog
        open={!!confirm}
        onOpenChange={o => {
          if (!o) setConfirm(null)
        }}
        title={confirm?.title}
        description={confirm?.description}
        confirmLabel={confirm?.confirmLabel}
        onConfirm={() => confirm?.onConfirm?.()}
      />
    </div>
  )
}
