import { useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { CH_COLORS } from '../../constants'
import LibraryFile from './LibraryFile'
import ImportModal from './ImportModal'

export default function LibraryTab({ cases, setCases, search, setSearch, labels, onReexport }) {
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
      const sw = await invoke('detect_cat_software_cmd')
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

  const filtered = cases
    .filter(c => showArchived ? c.archived : !c.archived)
    .filter(c => {
      if (!search.trim()) return true
      const q = search.toLowerCase()
      return c.name.toLowerCase().includes(q) ||
        c.sessions.some(s => s.sourceName?.toLowerCase().includes(q) ||
          s.participants.some(p => p.label.toLowerCase().includes(q)))
    })

  const toggleCase = (id) => setExpandedCases(p => ({...p, [id]: !p[id]}))

  const deleteCase = async (id) => {
    if (!confirm('Delete this case and all its session records? (Files on disk are not deleted.)')) return
    await invoke('library_delete_case', { caseId: id })
    setCases(p => p.filter(c => c.id !== id))
  }
  const archiveCase = async (id, archived) => {
    await invoke('library_archive_case', { caseId: id, archived })
    setCases(p => p.map(c => c.id === id ? {...c, archived} : c))
  }
  const renameCase = async (id) => {
    if (!editName.trim()) return
    await invoke('library_rename_case', { caseId: id, name: editName.trim() })
    setCases(p => p.map(c => c.id === id ? {...c, name: editName.trim()} : c))
    setEditingCase(null)
  }
  const deleteSession = async (caseId, sessionId) => {
    await invoke('library_delete_session', { caseId, sessionId })
    setCases(p => p.map(c => c.id === caseId ? {...c, sessions: c.sessions.filter(s => s.id !== sessionId)} : c))
  }

  const handleImportDone = () => {
    setImportModal(false)
    invoke('library_get').then(setCases).catch(() => {})
  }

  return (
    <div className="lib-wrap">
      <div className="lib-toolbar">
        <div className="lib-search-wrap">
          <svg width="14" height="14" viewBox="0 0 14 14" fill="none" className="lib-search-icon">
            <circle cx="6" cy="6" r="4.5" stroke="currentColor" strokeWidth="1.3"/>
            <line x1="9.5" y1="9.5" x2="13" y2="13" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round"/>
          </svg>
          <input className="lib-search" placeholder="Search cases, participants…" value={search} onChange={e => setSearch(e.target.value)} />
          {search && <button className="lib-search-clear" onClick={() => setSearch('')}>×</button>}
        </div>
        <button className={`ghost-btn${showArchived?' ghost-btn--active':''}`} onClick={() => setShowArchived(p => !p)}>
          {showArchived ? 'Hide archived' : 'Show archived'}
        </button>
        <button className="btn btn--sm lib-import-btn" onClick={() => setImportModal(true)}>
          <svg width="12" height="12" viewBox="0 0 12 12" fill="none" style={{flexShrink:0}}>
            <path d="M6 1v7M3 5l3 3 3-3M1 10h10" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"/>
          </svg>
          Import Audio
        </button>
        <button className="btn btn--sm" onClick={detectSoftware} disabled={scanningCat}>
          {scanningCat ? 'Scanning…' : 'Detect Software'}
        </button>
      </div>

      {/* CAT Software Detection Results */}
      {catSoftware !== null && (
        <div className="cat-detect-section">
          {catSoftware.length === 0 ? (
            <p className="cat-empty">No court reporting software found on this machine.</p>
          ) : (
            <>
              <div className="cat-found">
                {catSoftware.map((sw, i) => (
                  <div key={i} className="cat-sw-chip" onClick={async () => {
                    const jobs = await invoke('scan_cat_jobs_cmd', { path: sw.path })
                    setCatJobs(jobs)
                  }}>
                    <span className="cat-sw-name">{sw.name}</span>
                    <span className="cat-sw-count">{sw.jobCount} file{sw.jobCount !== 1 ? 's' : ''}</span>
                  </div>
                ))}
              </div>
              {catJobs.length > 0 && (
                <div className="cat-jobs">
                  <span className="cat-jobs-label">Available for import:</span>
                  {catJobs.slice(0, 20).map((job, i) => (
                    <div key={i} className="cat-job-row" onClick={() => onReexport(job.files[0]?.path, job.name)}>
                      <span className="cat-job-name">{job.name}</span>
                      <span className="cat-job-sw">{job.software}</span>
                      <span className="cat-job-files">{job.files.length} file{job.files.length !== 1 ? 's' : ''}</span>
                      <span className="cat-job-date">{job.dateModified}</span>
                    </div>
                  ))}
                </div>
              )}
            </>
          )}
        </div>
      )}

      {filtered.length === 0 && !importModal && (
        <div className="lib-empty">
          <svg width="48" height="48" viewBox="0 0 48 48" fill="none" className="lib-empty-icon">
            <rect x="8" y="12" width="32" height="28" rx="3" stroke="currentColor" strokeWidth="1.5"/>
            <path d="M8 20h32" stroke="currentColor" strokeWidth="1.5"/>
            <path d="M18 12V8a2 2 0 012-2h8a2 2 0 012 2v4" stroke="currentColor" strokeWidth="1.5"/>
          </svg>
          <p className="lib-empty-title">{search ? 'No matches' : cases.length === 0 ? 'No cases yet' : 'No cases to show'}</p>
          <p className="lib-empty-sub">
            {search ? 'Try a different search' : 'Convert a recording or use Import Audio to add files directly'}
          </p>
        </div>
      )}

      <div className="lib-list">
        {filtered.map(c => (
          <div key={c.id} className={`lib-case${c.archived?' lib-case--archived':''}`}>
            <div className="lib-case-header" onClick={() => toggleCase(c.id)}>
              <div className="lib-case-chevron">{expandedCases[c.id] ? '▾' : '▸'}</div>
              <div className="lib-case-info">
                {editingCase === c.id ? (
                  <div className="lib-rename-row" onClick={e => e.stopPropagation()}>
                    <input className="lib-rename-input" value={editName} autoFocus
                      onChange={e => setEditName(e.target.value)}
                      onKeyDown={e => { if (e.key === 'Enter') renameCase(c.id); if (e.key === 'Escape') setEditingCase(null) }} />
                    <button className="btn btn--sm" onClick={() => renameCase(c.id)}>Save</button>
                    <button className="ghost-btn" onClick={() => setEditingCase(null)}>Cancel</button>
                  </div>
                ) : (
                  <span className="lib-case-name">{c.name}</span>
                )}
                <span className="lib-case-meta">
                  {c.sessions.length} session{c.sessions.length!==1?'s':''} · {new Date(c.createdAt).toLocaleDateString()}
                  {c.archived && <span className="lib-archived-tag">archived</span>}
                </span>
              </div>
              <div className="lib-case-actions" onClick={e => e.stopPropagation()}>
                <button className="lib-action-btn" title="Rename" onClick={() => { setEditingCase(c.id); setEditName(c.name) }}>✎</button>
                <button className="lib-action-btn" title={c.archived?'Unarchive':'Archive'} onClick={() => archiveCase(c.id, !c.archived)}>{c.archived ? '↩' : '⊙'}</button>
                <button className="lib-action-btn lib-action-btn--del" title="Delete" onClick={() => deleteCase(c.id)}>✕</button>
              </div>
            </div>

            {expandedCases[c.id] && (
              <div className="lib-sessions">
                {c.sessions.map(s => (
                  <div key={s.id} className="lib-session">
                    <div className="lib-session-header">
                      <span className="lib-session-date">{s.date}</span>
                      <span className="lib-session-src" title={s.sourceFile}>{s.sourceName}</span>
                      <div className="lib-session-actions">
                        <button className="lib-action-btn" title="Re-export source" onClick={() => onReexport(s.sourceFile, c.name)}>⟳ Re-export</button>
                        <button className="lib-action-btn lib-action-btn--del" title="Remove session" onClick={() => deleteSession(c.id, s.id)}>✕</button>
                      </div>
                    </div>
                    <div className="lib-participants">
                      {s.participants.map((p, pi) => (
                        <div key={pi} className="lib-participant">
                          <span className="lib-participant-dot" style={{background: CH_COLORS[pi%4]}} />
                          <span className="lib-participant-label">{p.label}</span>
                          <div className="lib-participant-files">
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
