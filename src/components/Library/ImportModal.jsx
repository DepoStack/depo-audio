import { useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { open as openDialog } from '@tauri-apps/plugin-dialog'
import * as Dialog from '@radix-ui/react-dialog'
import { basename } from '../../utils'
import Spinner from '../common/Spinner'

export default function ImportModal({ defaultLabels, existingCases, onDone, onClose }) {
  const [selectedFiles, setSelectedFiles] = useState([])
  const [caseName, setCaseName]           = useState('')
  const [label, setLabel]                 = useState(defaultLabels[0] || 'Reporter')
  const [customLabel, setCustomLabel]     = useState('')
  const [saving, setSaving]               = useState(false)
  const [error, setError]                 = useState('')
  const [caseInputMode, setCaseInputMode] = useState('existing')

  const labelValue = label === '__custom__' ? customLabel : label
  const allLabels  = [...defaultLabels, '__custom__']

  const browsePick = async () => {
    const selected = await openDialog({ multiple: true, filters: [
      { name: 'Audio', extensions: ['wav','mp3','flac','aac','ogg','opus','wma','m4a','aif','aiff'] },
      { name: 'All Files', extensions: ['*'] }
    ]}).catch(() => null)
    if (!selected) return
    const paths = Array.isArray(selected) ? selected : [selected]
    setSelectedFiles(paths)
    if (!caseName && paths.length > 0) {
      const detected = await invoke('infer_case_name_cmd', { filename: basename(paths[0]) }).catch(() => '')
      if (detected) { setCaseName(detected); setCaseInputMode('new') }
    }
    setError('')
  }

  const handleSave = async () => {
    if (!selectedFiles.length) { setError('Select at least one file'); return }
    const cn = caseName.trim()
    if (!cn) { setError('Enter a case name'); return }
    const lbl = labelValue.trim()
    if (!lbl) { setError('Enter a speaker label'); return }
    setSaving(true); setError('')
    try {
      for (const path of selectedFiles) {
        await invoke('library_import_file', { path, caseName: cn, label: lbl })
      }
      onDone()
    } catch(e) {
      setError(String(e))
      setSaving(false)
    }
  }

  return (
    <Dialog.Root open onOpenChange={open => { if (!open) onClose() }}>
      <Dialog.Portal>
        <Dialog.Overlay className="modal-overlay" />
        <Dialog.Content className="modal" onPointerDownOutside={onClose}>
          <div className="modal-head">
            <Dialog.Title className="modal-title">Import Audio to Library</Dialog.Title>
            <Dialog.Close className="modal-close">×</Dialog.Close>
          </div>
        <p className="modal-sub">Add already-converted or existing audio files directly to your library — no conversion needed.</p>

        {/* File picker */}
        <div className="modal-field">
          <label className="modal-label">FILES</label>
          <div className="modal-file-row">
            <button className="btn btn--sm" onClick={browsePick}>Browse files…</button>
            {selectedFiles.length > 0 && (
              <span className="modal-file-count">{selectedFiles.length} file{selectedFiles.length !== 1 ? 's' : ''} selected</span>
            )}
          </div>
          {selectedFiles.length > 0 && (
            <div className="modal-file-list">
              {selectedFiles.map((p, i) => (
                <div key={i} className="modal-file-item">
                  <span className="modal-file-name">{basename(p)}</span>
                  <button className="modal-file-remove" onClick={() => setSelectedFiles(f => f.filter((_,j) => j !== i))}>×</button>
                </div>
              ))}
            </div>
          )}
        </div>

        {/* Case */}
        <div className="modal-field">
          <label className="modal-label">CASE NAME</label>
          <div className="modal-case-row">
            {existingCases.length > 0 && (
              <div className="modal-case-tabs">
                <button className={`modal-case-tab${caseInputMode==='existing'?' active':''}`} onClick={() => setCaseInputMode('existing')}>Existing case</button>
                <button className={`modal-case-tab${caseInputMode==='new'?' active':''}`} onClick={() => setCaseInputMode('new')}>New case</button>
              </div>
            )}
            {(caseInputMode === 'existing' && existingCases.length > 0) ? (
              <select className="opt-select" value={caseName} onChange={e => setCaseName(e.target.value)}>
                <option value="">— select a case —</option>
                {existingCases.map(n => <option key={n} value={n}>{n}</option>)}
              </select>
            ) : (
              <input className="opt-input" value={caseName} placeholder="e.g. Smith v. Metro Transit"
                onChange={e => setCaseName(e.target.value)} />
            )}
          </div>
        </div>

        {/* Speaker label */}
        <div className="modal-field">
          <label className="modal-label">SPEAKER / PARTICIPANT</label>
          <div className="modal-label-row">
            <div className="modal-label-chips">
              {defaultLabels.map(l => (
                <button key={l} className={`modal-chip${label===l?' modal-chip--active':''}`}
                  onClick={() => { setLabel(l); setCustomLabel('') }}>{l}</button>
              ))}
              <button className={`modal-chip${label==='__custom__'?' modal-chip--active':''}`}
                onClick={() => setLabel('__custom__')}>Custom…</button>
            </div>
            {label === '__custom__' && (
              <input className="opt-input" style={{marginTop:8}} value={customLabel}
                placeholder="Enter speaker name or role…"
                onChange={e => setCustomLabel(e.target.value)} />
            )}
          </div>
          <p className="modal-field-note">All selected files will be filed under this label. Import multiple times for multiple speakers.</p>
        </div>

        {error && <p className="modal-error">{error}</p>}

        <div className="modal-actions">
          <Dialog.Close asChild>
            <button className="ghost-btn" disabled={saving}>Cancel</button>
          </Dialog.Close>
          <button className="btn btn--primary" onClick={handleSave} disabled={saving}>
            {saving ? <><Spinner />Saving…</> : `Import ${selectedFiles.length > 0 ? selectedFiles.length + ' file' + (selectedFiles.length !== 1 ? 's' : '') : ''}`}
          </button>
        </div>
      </Dialog.Content>
    </Dialog.Portal>
    </Dialog.Root>
  )
}
