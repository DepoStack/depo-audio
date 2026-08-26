import { useEffect, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { open as openDialog } from '@tauri-apps/plugin-dialog'
import { Loader2, X } from 'lucide-react'
import { cn } from '../../lib/utils'
import { basename } from '../../utils'
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '../ui/dialog'
import { Button } from '../ui/button'
import { Input } from '../ui/input'
import { Label } from '../ui/label'

export default function ImportModal({ defaultLabels, existingCases, onDone, onClose, onStorageError }) {
  const [selectedFiles, setSelectedFiles] = useState([])
  const [caseName, setCaseName] = useState('')
  const [label, setLabel] = useState(defaultLabels[0] || 'Reporter')
  const [customLabel, setCustomLabel] = useState('')
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')
  const [errorDetail, setErrorDetail] = useState('')
  const [caseInputMode, setCaseInputMode] = useState('existing')
  const errorRef = useRef(null)

  const labelValue = label === '__custom__' ? customLabel : label

  useEffect(() => {
    if (error) errorRef.current?.focus()
  }, [error])

  const browsePick = async () => {
    let selected
    try {
      selected = await openDialog({
        multiple: true,
        filters: [
          { name: 'Audio', extensions: ['wav', 'mp3', 'flac', 'aac', 'ogg', 'opus', 'wma', 'm4a', 'aif', 'aiff'] },
          { name: 'All Files', extensions: ['*'] },
        ],
      })
    } catch (dialogError) {
      setError('The file picker could not be opened. Try again, or restart DepoAudio if the problem continues.')
      setErrorDetail(String(dialogError))
      return
    }
    if (!selected) return
    const paths = Array.isArray(selected) ? selected : [selected]
    setSelectedFiles(paths)
    if (!caseName && paths.length > 0) {
      const detected = await invoke('infer_case_name_cmd', { filename: basename(paths[0]) }).catch(() => '')
      if (detected) {
        setCaseName(detected)
        setCaseInputMode('new')
      }
    }
    setError('')
    setErrorDetail('')
  }

  const handleSave = async () => {
    if (!selectedFiles.length) {
      setError('Select at least one file')
      setErrorDetail('')
      return
    }
    const cn = caseName.trim()
    if (!cn) {
      setError('Enter a case name')
      setErrorDetail('')
      return
    }
    const lbl = labelValue.trim()
    if (!lbl) {
      setError('Enter a speaker label')
      setErrorDetail('')
      return
    }
    setSaving(true)
    setError('')
    setErrorDetail('')
    try {
      const outcome = await invoke('library_import_files', { paths: selectedFiles, caseName: cn, label: lbl })
      onDone(outcome?.warning || '')
    } catch (e) {
      setError('Audio could not be imported. Existing Library data remains unchanged.')
      setErrorDetail(String(e))
      onStorageError?.(e)
      setSaving(false)
    }
  }

  return (
    <Dialog
      open
      onOpenChange={open => {
        if (!open && !saving) onClose()
      }}
    >
      <DialogContent
        aria-busy={saving}
        onPointerDownOutside={event => {
          if (saving) event.preventDefault()
        }}
        onEscapeKeyDown={event => {
          if (saving) event.preventDefault()
        }}
      >
        <DialogHeader>
          <DialogTitle>Import Audio to Library</DialogTitle>
          <DialogClose asChild>
            <button
              type="button"
              disabled={saving}
              aria-label="Close import dialog"
              className="rounded-sm p-1 text-[hsl(var(--sub))] hover:text-foreground transition-colors disabled:opacity-40 focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1"
            >
              <X size={16} aria-hidden="true" />
            </button>
          </DialogClose>
        </DialogHeader>

        <DialogDescription className="px-5 pt-2.5 text-xs text-[hsl(var(--sub))] leading-relaxed">
          Add already-converted or existing audio files directly to your library — no conversion needed.
        </DialogDescription>

        {/* File picker */}
        <fieldset className="min-w-0 px-5 pt-3.5">
          <legend className="font-mono text-[9px] font-medium tracking-[1.2px] uppercase text-[hsl(var(--sub))]">
            Files
          </legend>
          <div className="flex items-center gap-2.5 mt-1.5">
            <Button type="button" size="sm" onClick={browsePick}>
              Browse files…
            </Button>
            {selectedFiles.length > 0 && (
              <span role="status" aria-live="polite" className="text-[11px] text-[hsl(var(--sub))]">
                {selectedFiles.length} file{selectedFiles.length !== 1 ? 's' : ''} selected
              </span>
            )}
          </div>
          {selectedFiles.length > 0 && (
            <div className="mt-2 flex flex-col gap-0.5 max-h-[100px] overflow-y-auto">
              {selectedFiles.map((p, i) => (
                <div
                  key={i}
                  className="flex items-center justify-between px-2.5 py-1 bg-secondary rounded-md text-[11px] text-[hsl(var(--text2))]"
                >
                  <span className="truncate min-w-0">{basename(p)}</span>
                  <button
                    type="button"
                    aria-label={`Remove ${basename(p)} from the import`}
                    className="text-[hsl(var(--sub))] hover:text-destructive transition-colors shrink-0 ml-2 rounded-sm p-1 focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1"
                    onClick={() => setSelectedFiles(f => f.filter((_, j) => j !== i))}
                  >
                    <X size={12} aria-hidden="true" />
                  </button>
                </div>
              ))}
            </div>
          )}
        </fieldset>

        {/* Case */}
        <fieldset className="min-w-0 px-5 pt-3.5">
          <legend className="font-mono text-[9px] font-medium tracking-[1.2px] uppercase text-[hsl(var(--sub))]">
            Case name
          </legend>
          <div className="mt-1.5">
            {existingCases.length > 0 && (
              <div role="group" aria-label="Case type" className="flex gap-0.5 bg-secondary rounded-md p-0.5 mb-2">
                <button
                  type="button"
                  aria-pressed={caseInputMode === 'existing'}
                  className={cn(
                    'flex-1 px-3 py-1.5 rounded-md text-xs font-semibold transition-colors focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1',
                    caseInputMode === 'existing'
                      ? 'bg-[hsl(var(--gold-dim))] text-foreground'
                      : 'text-[hsl(var(--sub))] hover:text-[hsl(var(--text2))]',
                  )}
                  onClick={() => setCaseInputMode('existing')}
                >
                  Existing case
                </button>
                <button
                  type="button"
                  aria-pressed={caseInputMode === 'new'}
                  className={cn(
                    'flex-1 px-3 py-1.5 rounded-md text-xs font-semibold transition-colors focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1',
                    caseInputMode === 'new'
                      ? 'bg-[hsl(var(--gold-dim))] text-foreground'
                      : 'text-[hsl(var(--sub))] hover:text-[hsl(var(--text2))]',
                  )}
                  onClick={() => setCaseInputMode('new')}
                >
                  New case
                </button>
              </div>
            )}
            {caseInputMode === 'existing' && existingCases.length > 0 ? (
              <>
                <Label htmlFor="library-import-existing-case" className="sr-only">
                  Existing case
                </Label>
                <select
                  id="library-import-existing-case"
                  className="flex h-8 w-full rounded-lg border border-border bg-card px-2.5 py-1.5 text-xs text-foreground transition-colors focus:outline-hidden focus:border-primary focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1"
                  value={caseName}
                  onChange={e => setCaseName(e.target.value)}
                >
                  <option value="">— select a case —</option>
                  {existingCases.map(n => (
                    <option key={n} value={n}>
                      {n}
                    </option>
                  ))}
                </select>
              </>
            ) : (
              <>
                <Label htmlFor="library-import-new-case" className="sr-only">
                  New case name
                </Label>
                <Input
                  id="library-import-new-case"
                  value={caseName}
                  placeholder="e.g. Smith v. Metro Transit"
                  onChange={e => setCaseName(e.target.value)}
                />
              </>
            )}
          </div>
        </fieldset>

        {/* Speaker label */}
        <fieldset className="min-w-0 px-5 pt-3.5">
          <legend className="font-mono text-[9px] font-medium tracking-[1.2px] uppercase text-[hsl(var(--sub))]">
            Speaker or participant
          </legend>
          <div className="mt-1.5">
            <div role="group" aria-label="Speaker or participant label" className="flex flex-wrap gap-1.5">
              {defaultLabels.map(l => (
                <Button
                  key={l}
                  type="button"
                  aria-pressed={label === l}
                  variant="outline"
                  size="sm"
                  className={cn(
                    'rounded-full',
                    label === l && 'bg-[hsl(var(--gold-dim))] text-foreground border-primary/30',
                  )}
                  onClick={() => {
                    setLabel(l)
                    setCustomLabel('')
                  }}
                >
                  {l}
                </Button>
              ))}
              <Button
                type="button"
                aria-pressed={label === '__custom__'}
                variant="outline"
                size="sm"
                className={cn(
                  'rounded-full',
                  label === '__custom__' && 'bg-[hsl(var(--gold-dim))] text-foreground border-primary/30',
                )}
                onClick={() => setLabel('__custom__')}
              >
                Custom…
              </Button>
            </div>
            {label === '__custom__' && (
              <>
                <Label htmlFor="library-import-custom-label" className="sr-only">
                  Custom speaker or participant
                </Label>
                <Input
                  id="library-import-custom-label"
                  className="mt-2"
                  value={customLabel}
                  placeholder="Enter speaker name or role…"
                  onChange={e => setCustomLabel(e.target.value)}
                />
              </>
            )}
          </div>
          <p className="mt-1.5 text-[10px] text-[hsl(var(--sub))] leading-relaxed">
            All selected files will be filed under this label. Import multiple times for multiple speakers.
          </p>
        </fieldset>

        {error && (
          <div
            ref={errorRef}
            role="alert"
            tabIndex={-1}
            className="mx-5 mt-2.5 px-3 py-2 bg-destructive/10 border border-destructive/20 rounded-md text-xs text-destructive focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1"
          >
            <p>{error}</p>
            {errorDetail && (
              <details className="mt-1.5 text-foreground">
                <summary className="w-fit cursor-pointer font-semibold focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1">
                  Technical details
                </summary>
                <p className="mt-1 whitespace-pre-wrap break-words text-[10px] text-[hsl(var(--sub))]">{errorDetail}</p>
              </details>
            )}
          </div>
        )}

        <DialogFooter>
          <DialogClose asChild>
            <Button type="button" variant="ghost" disabled={saving}>
              Cancel
            </Button>
          </DialogClose>
          <Button
            type="button"
            variant="primary"
            onClick={handleSave}
            disabled={saving}
            aria-describedby="import-status"
          >
            {saving ? (
              <>
                <Loader2 className="h-3.5 w-3.5 animate-spin" aria-hidden="true" />
                Saving…
              </>
            ) : (
              `Import ${selectedFiles.length > 0 ? selectedFiles.length + ' file' + (selectedFiles.length !== 1 ? 's' : '') : ''}`
            )}
          </Button>
          <span id="import-status" role="status" aria-live="polite" className="sr-only">
            {saving ? 'Saving imported audio to the Library.' : ''}
          </span>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
