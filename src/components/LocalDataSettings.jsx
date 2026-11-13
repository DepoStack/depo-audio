import { useCallback, useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { open, save } from '@tauri-apps/plugin-dialog'
import { AlertCircle, FolderOpen, Download, Upload, Trash2, RotateCw } from 'lucide-react'
import { Button } from './ui/button'
import { Card, CardHeader, CardTitle, CardContent } from './ui/card'
import { ConfirmDialog } from './ui/confirm-dialog'
import { clearWebData, collectWebData, defaultBackupName, restoreWebData, summarizeWebData } from '../lib/localData'

// ── Your data (DA-07) ─────────────────────────────────────────────────────────
//
// Where DepoAudio keeps its own data, a zip backup of the library and
// transcripts with import, and "Clear all DepoAudio data". Everything here is
// local: no account, no network. Recordings, converted files, export records
// and backup zips are never deleted.

const ITEM_DESCRIPTIONS = {
  library: 'Case library: case names, sessions and where converted files are',
  prefs: 'Settings',
  'unfinished-save': 'Unfinished save left by an interrupted write',
  'legacy-model': 'Legacy model file from an older build',
}

const plural = (count, word) => `${count} ${word}${count === 1 ? '' : 's'}`

function formatBytes(bytes) {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}

function webStorage() {
  try {
    return window.localStorage
  } catch {
    return null
  }
}

function joinSentence(parts) {
  if (parts.length <= 1) return parts.join('')
  return `${parts.slice(0, -1).join(', ')} and ${parts[parts.length - 1]}`
}

function importSummary(result, restored) {
  const lines = [
    `${plural(result.casesAdded, 'case')} and ${plural(result.sessionsAdded, 'session')} added to the Library.`,
  ]
  if (result.sessionsSkipped > 0) {
    lines.push(
      `${result.sessionsSkipped} ${result.sessionsSkipped === 1 ? 'session was' : 'sessions were'} already here and left as ${result.sessionsSkipped === 1 ? 'it is' : 'they are'}.`,
    )
  }
  lines.push(
    `${plural(restored.transcriptsAdded, 'transcript')} added, ${plural(restored.bookmarksAdded, 'bookmark')} added.`,
  )
  if (restored.transcriptsKept > 0) {
    lines.push(
      `${restored.transcriptsKept} ${restored.transcriptsKept === 1 ? 'transcript on this computer was' : 'transcripts on this computer were'} kept because ${restored.transcriptsKept === 1 ? 'it differs' : 'they differ'} from the backup.`,
    )
  }
  if (result.filesUnavailable > 0) {
    lines.push(
      `${plural(result.filesUnavailable, 'file')} in the backup ${result.filesUnavailable === 1 ? 'is' : 'are'} not on this computer; the Library entries are kept so you can see them.`,
    )
  }
  if (result.filesNeedRestart > 0) {
    lines.push(
      `${plural(result.filesNeedRestart, 'file')} in the backup ${result.filesNeedRestart === 1 ? 'is' : 'are'} on this computer but ${result.filesNeedRestart === 1 ? 'was' : 'were'} removed from playback earlier in this app session. Restart DepoAudio to play ${result.filesNeedRestart === 1 ? 'it' : 'them'}; the Library entries are kept.`,
    )
  }
  if (restored.failed > 0) {
    lines.push(`${plural(restored.failed, 'item')} could not be saved to this app's storage.`)
  }
  return lines
}

export default function LocalDataSettings({
  reloadApp = () => window.location.reload(),
  onReloadNeeded,
  conversionActive = false,
}) {
  const [info, setInfo] = useState(null)
  const [infoError, setInfoError] = useState('')
  const [web, setWeb] = useState(() => summarizeWebData(webStorage()))
  const [busy, setBusy] = useState('')
  const [message, setMessage] = useState('')
  const [error, setError] = useState('')
  const [importLines, setImportLines] = useState(null)
  const [confirmClear, setConfirmClear] = useState(false)

  const loadInfo = useCallback(
    () =>
      invoke('local_data_info_cmd')
        .then(value => {
          setInfo(value)
          setInfoError('')
        })
        .catch(loadError => setInfoError(`Couldn't read where DepoAudio keeps its data: ${String(loadError)}`)),
    [],
  )

  useEffect(() => {
    loadInfo()
  }, [loadInfo])

  const reset = () => {
    setMessage('')
    setError('')
    setImportLines(null)
  }

  const showFolder = async () => {
    reset()
    try {
      await invoke('show_in_folder', { path: info.appDataDir })
    } catch (showError) {
      setError(`Couldn't open the folder: ${String(showError)}`)
    }
  }

  const exportBackup = async () => {
    reset()
    setBusy('export')
    try {
      const path = await save({
        defaultPath: defaultBackupName(),
        filters: [{ name: 'DepoAudio backup', extensions: ['zip'] }],
      })
      if (!path) return
      const { transcripts, bookmarks } = collectWebData(webStorage())
      const counts = await invoke('local_data_export_cmd', { path, transcripts, bookmarks })
      setMessage(
        `Saved a backup with ${joinSentence([
          plural(counts.cases, 'case'),
          plural(counts.sessions, 'session'),
          plural(counts.transcripts, 'transcript'),
          plural(counts.bookmarks, 'bookmark'),
        ])}. It is not encrypted; keep it where you keep the recordings.`,
      )
    } catch (exportError) {
      setError(`The backup was not saved: ${String(exportError)}`)
    } finally {
      setBusy('')
    }
  }

  const importBackup = async () => {
    reset()
    setBusy('import')
    try {
      const selected = await open({
        multiple: false,
        filters: [{ name: 'DepoAudio backup', extensions: ['zip'] }],
      })
      if (!selected) return
      const path = typeof selected === 'string' ? selected : selected.path
      const result = await invoke('local_data_import_cmd', { path })
      const restored = restoreWebData(webStorage(), result)
      setImportLines(importSummary(result, restored))
      setWeb(summarizeWebData(webStorage()))
      onReloadNeeded?.()
      loadInfo()
    } catch (importError) {
      setError(`Nothing was imported: ${String(importError)}`)
    } finally {
      setBusy('')
    }
  }

  const clearAll = async () => {
    reset()
    setBusy('clear')
    try {
      const result = await invoke('local_data_clear_cmd')
      const webResult = clearWebData(webStorage())
      const warnings = [
        result?.warning,
        webResult.failed > 0 ? `${plural(webResult.failed, 'stored item')} could not be removed.` : '',
      ]
        .filter(Boolean)
        .join(' ')
      if (!warnings) {
        reloadApp()
        return
      }
      setError(`Some DepoAudio data was not cleared. ${warnings}`)
      setWeb(summarizeWebData(webStorage()))
      onReloadNeeded?.()
      loadInfo()
    } catch (clearError) {
      setError(`Nothing was cleared: ${String(clearError)}`)
    } finally {
      setBusy('')
    }
  }

  const libraryReadable = info?.libraryReadable !== false

  return (
    <>
      <Card>
        <CardHeader>
          <CardTitle>Where DepoAudio keeps your data</CardTitle>
        </CardHeader>
        <CardContent className="p-4 flex flex-col gap-3 text-[11px]">
          {infoError && (
            <p role="alert" className="rounded-md bg-destructive/10 px-3 py-2 text-destructive">
              {infoError}
            </p>
          )}
          {info && (
            <div className="flex flex-col gap-2">
              <div className="flex items-start justify-between gap-3">
                <div className="flex flex-col min-w-0">
                  <span className="text-[12px] font-medium text-foreground">App data folder</span>
                  <code className="mt-0.5 break-all font-mono text-[10.5px] text-[hsl(var(--text2))]">
                    {info.appDataDir}
                  </code>
                </div>
                <Button
                  size="sm"
                  variant="outline"
                  className="shrink-0"
                  onClick={showFolder}
                  aria-label="Show the app data folder"
                >
                  <FolderOpen size={12} /> Show
                </Button>
              </div>
              {info.items.length === 0 ? (
                <p className="text-[hsl(var(--sub))]">Nothing is saved in this folder yet.</p>
              ) : (
                <ul className="flex flex-col gap-1">
                  {info.items.map(item => (
                    <li key={item.name} className="flex items-baseline justify-between gap-3">
                      <span className="min-w-0">
                        <code className="font-mono text-[10.5px] text-foreground">{item.name}</code>
                        <span className="text-[hsl(var(--sub))]">
                          {' '}
                          — {ITEM_DESCRIPTIONS[item.kind] ?? 'DepoAudio file'}
                        </span>
                      </span>
                      <span className="shrink-0 font-mono text-[10px] text-[hsl(var(--sub))]">
                        {formatBytes(item.bytes)}
                      </span>
                    </li>
                  ))}
                </ul>
              )}
            </div>
          )}

          <div className="flex flex-col border-t border-border pt-3">
            <span className="text-[12px] font-medium text-foreground">App web storage</span>
            <span className="mt-0.5 text-[hsl(var(--sub))]">
              {plural(web.transcripts, 'track transcript')} you edited in the Player,{' '}
              {plural(web.bookmarks, 'bookmark')}, playback speed and theme. The system web view keeps them for
              DepoAudio on this computer.
            </span>
          </div>

          <p className="border-t border-border pt-3 text-[hsl(var(--text2))]">
            Your recordings, converted files and their export records stay where you saved them; DepoAudio never moves
            or deletes them. Nothing here is uploaded, and no account is needed. DepoAudio does not encrypt these files
            itself: turn on full-disk encryption (FileVault on macOS, BitLocker or Device Encryption on Windows) to
            protect them.
          </p>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Back up and restore</CardTitle>
        </CardHeader>
        <CardContent className="p-4 flex flex-col gap-3 text-[11px]">
          <div className="flex items-center justify-between gap-3">
            <div className="flex flex-col min-w-0">
              <span className="text-[12px] font-medium text-foreground">Export library and transcripts</span>
              <span className="mt-0.5 text-[hsl(var(--sub))]">
                {libraryReadable
                  ? 'Saves a new .zip with your case library, transcripts and bookmarks. It holds no audio and is not encrypted.'
                  : 'The library could not be loaded, so it cannot be exported. Clearing all DepoAudio data starts a new library.'}
              </span>
            </div>
            <Button
              size="sm"
              variant="outline"
              className="shrink-0"
              onClick={exportBackup}
              disabled={!!busy || !libraryReadable || !info}
              aria-label="Export library and transcripts"
            >
              <Download size={12} /> Export…
            </Button>
          </div>

          <div className="flex items-center justify-between gap-3 border-t border-border pt-3">
            <div className="flex flex-col min-w-0">
              <span className="text-[12px] font-medium text-foreground">Import a backup</span>
              <span className="mt-0.5 text-[hsl(var(--sub))]">
                Adds the cases, transcripts and bookmarks from a DepoAudio backup. Nothing you have now is removed or
                replaced. Transcripts follow the track&apos;s file location, so they appear when the files are in the
                same place.
              </span>
            </div>
            <Button
              size="sm"
              variant="outline"
              className="shrink-0"
              onClick={importBackup}
              disabled={!!busy}
              aria-label="Import a backup"
            >
              <Upload size={12} /> Import…
            </Button>
          </div>

          {message && (
            <p role="status" className="rounded-md border border-border bg-secondary/30 px-3 py-2 text-foreground">
              {message}
            </p>
          )}
          {importLines && (
            <div
              role="status"
              aria-label="Import result"
              className="flex flex-col gap-2 rounded-md border border-border bg-secondary/30 px-3 py-2 text-foreground"
            >
              <ul className="flex flex-col gap-0.5">
                {importLines.map(line => (
                  <li key={line}>{line}</li>
                ))}
              </ul>
              <div className="flex items-center justify-between gap-3">
                <span className="text-[hsl(var(--sub))]">Reload so every screen shows the imported data.</span>
                <Button size="sm" variant="primary" className="shrink-0" onClick={() => reloadApp()}>
                  <RotateCw size={12} /> Reload DepoAudio
                </Button>
              </div>
            </div>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Clear all DepoAudio data</CardTitle>
        </CardHeader>
        <CardContent className="p-4 flex items-center justify-between gap-3 text-[11px]">
          <span className="min-w-0 text-[hsl(var(--sub))]">
            Removes the case library, settings, transcripts, bookmarks and the other files listed above from this
            computer. Recordings, converted files, export records and backups are not deleted. Export a backup first if
            you may need this later.
            {conversionActive && (
              <span className="mt-1 block text-foreground">
                Clear all is not available while files are converting. Wait for them to finish or cancel them.
              </span>
            )}
          </span>
          <Button
            size="sm"
            variant="outline"
            className="shrink-0 hover:text-destructive"
            onClick={() => setConfirmClear(true)}
            disabled={!!busy || conversionActive}
          >
            <Trash2 size={12} /> Clear all DepoAudio data…
          </Button>
        </CardContent>
      </Card>

      {error && (
        <p
          role="alert"
          className="flex items-start gap-2 rounded-md bg-destructive/10 px-3 py-2 text-[11px] text-destructive"
        >
          <AlertCircle size={13} className="mt-0.5 shrink-0" /> {error}
        </p>
      )}

      <ConfirmDialog
        open={confirmClear}
        onOpenChange={setConfirmClear}
        title="Clear all DepoAudio data?"
        description={`This deletes the case library, settings, ${plural(web.transcripts, 'transcript')} and ${plural(web.bookmarks, 'bookmark')} from this computer. It cannot be undone. Recordings, converted files, export records and backups are not deleted. DepoAudio then reloads as on first launch.`}
        confirmLabel="Clear all data"
        onConfirm={clearAll}
      />
    </>
  )
}
