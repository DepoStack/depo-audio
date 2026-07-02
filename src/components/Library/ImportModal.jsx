import { useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { open as openDialog } from '@tauri-apps/plugin-dialog'
import { Loader2, X } from 'lucide-react'
import { cn } from '../../lib/utils'
import { basename } from '../../utils'
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter, DialogClose } from '../ui/dialog'
import { Button } from '../ui/button'
import { Input } from '../ui/input'
import { Label } from '../ui/label'

export default function ImportModal({ defaultLabels, existingCases, onDone, onClose }) {
  const [selectedFiles, setSelectedFiles] = useState([])
  const [caseName, setCaseName]           = useState('')
  const [label, setLabel]                 = useState(defaultLabels[0] || 'Reporter')
  const [customLabel, setCustomLabel]     = useState('')
  const [saving, setSaving]               = useState(false)
  const [error, setError]                 = useState('')
  const [caseInputMode, setCaseInputMode] = useState('existing')

  const labelValue = label === '__custom__' ? customLabel : label

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
    <Dialog open onOpenChange={open => { if (!open) onClose() }}>
      <DialogContent onPointerDownOutside={onClose}>
        <DialogHeader>
          <DialogTitle>Import Audio to Library</DialogTitle>
          <DialogClose asChild>
            <button className="text-[hsl(var(--sub))] hover:text-foreground transition-colors">
              <X size={16} />
            </button>
          </DialogClose>
        </DialogHeader>

        <p className="px-5 pt-2.5 text-xs text-[hsl(var(--sub))] leading-relaxed">
          Add already-converted or existing audio files directly to your library — no conversion needed.
        </p>

        {/* File picker */}
        <div className="px-5 pt-3.5">
          <Label>FILES</Label>
          <div className="flex items-center gap-2.5 mt-1.5">
            <Button size="sm" onClick={browsePick}>Browse files…</Button>
            {selectedFiles.length > 0 && (
              <span className="text-[11px] text-[hsl(var(--sub))]">
                {selectedFiles.length} file{selectedFiles.length !== 1 ? 's' : ''} selected
              </span>
            )}
          </div>
          {selectedFiles.length > 0 && (
            <div className="mt-2 flex flex-col gap-0.5 max-h-[100px] overflow-y-auto">
              {selectedFiles.map((p, i) => (
                <div key={i} className="flex items-center justify-between px-2.5 py-1 bg-secondary rounded-md text-[11px] text-[hsl(var(--text2))]">
                  <span className="truncate min-w-0">{basename(p)}</span>
                  <button
                    className="text-[hsl(var(--sub))] hover:text-destructive transition-colors shrink-0 ml-2"
                    onClick={() => setSelectedFiles(f => f.filter((_, j) => j !== i))}
                  >
                    <X size={12} />
                  </button>
                </div>
              ))}
            </div>
          )}
        </div>

        {/* Case */}
        <div className="px-5 pt-3.5">
          <Label>CASE NAME</Label>
          <div className="mt-1.5">
            {existingCases.length > 0 && (
              <div className="flex gap-0.5 bg-secondary rounded-md p-0.5 mb-2">
                <button
                  className={cn(
                    'flex-1 px-3 py-1.5 rounded-md text-xs font-semibold transition-colors',
                    caseInputMode === 'existing'
                      ? 'bg-[hsl(var(--gold-dim))] text-primary'
                      : 'text-[hsl(var(--sub))] hover:text-[hsl(var(--text2))]'
                  )}
                  onClick={() => setCaseInputMode('existing')}
                >
                  Existing case
                </button>
                <button
                  className={cn(
                    'flex-1 px-3 py-1.5 rounded-md text-xs font-semibold transition-colors',
                    caseInputMode === 'new'
                      ? 'bg-[hsl(var(--gold-dim))] text-primary'
                      : 'text-[hsl(var(--sub))] hover:text-[hsl(var(--text2))]'
                  )}
                  onClick={() => setCaseInputMode('new')}
                >
                  New case
                </button>
              </div>
            )}
            {(caseInputMode === 'existing' && existingCases.length > 0) ? (
              <select
                className="flex h-8 w-full rounded-lg border border-border bg-card px-2.5 py-1.5 text-xs text-foreground transition-colors focus:outline-hidden focus:border-primary"
                aria-label="Case"
                value={caseName}
                onChange={e => setCaseName(e.target.value)}
              >
                <option value="">— select a case —</option>
                {existingCases.map(n => <option key={n} value={n}>{n}</option>)}
              </select>
            ) : (
              <Input value={caseName} placeholder="e.g. Smith v. Metro Transit"
                onChange={e => setCaseName(e.target.value)} />
            )}
          </div>
        </div>

        {/* Speaker label */}
        <div className="px-5 pt-3.5">
          <Label>SPEAKER / PARTICIPANT</Label>
          <div className="mt-1.5">
            <div className="flex flex-wrap gap-1.5">
              {defaultLabels.map(l => (
                <Button
                  key={l}
                  variant="outline"
                  size="sm"
                  className={cn(
                    'rounded-full',
                    label === l && 'bg-[hsl(var(--gold-dim))] text-primary border-primary/30'
                  )}
                  onClick={() => { setLabel(l); setCustomLabel('') }}
                >
                  {l}
                </Button>
              ))}
              <Button
                variant="outline"
                size="sm"
                className={cn(
                  'rounded-full',
                  label === '__custom__' && 'bg-[hsl(var(--gold-dim))] text-primary border-primary/30'
                )}
                onClick={() => setLabel('__custom__')}
              >
                Custom…
              </Button>
            </div>
            {label === '__custom__' && (
              <Input className="mt-2" value={customLabel}
                placeholder="Enter speaker name or role…"
                onChange={e => setCustomLabel(e.target.value)} />
            )}
          </div>
          <p className="mt-1.5 text-[10px] text-[hsl(var(--sub))] leading-relaxed">
            All selected files will be filed under this label. Import multiple times for multiple speakers.
          </p>
        </div>

        {error && (
          <p className="mx-5 mt-2.5 px-3 py-2 bg-destructive/10 border border-destructive/20 rounded-md text-xs text-destructive">
            {error}
          </p>
        )}

        <DialogFooter>
          <DialogClose asChild>
            <Button variant="ghost" disabled={saving}>Cancel</Button>
          </DialogClose>
          <Button variant="primary" onClick={handleSave} disabled={saving}>
            {saving
              ? <><Loader2 className="h-3.5 w-3.5 animate-spin" />Saving…</>
              : `Import ${selectedFiles.length > 0 ? selectedFiles.length + ' file' + (selectedFiles.length !== 1 ? 's' : '') : ''}`}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
