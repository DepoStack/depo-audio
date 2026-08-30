import { useState, useEffect, useRef, useCallback, lazy, Suspense } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { openUrl } from '@tauri-apps/plugin-opener'
import { Sun, Moon, Monitor, Settings, AudioLines, Play, FolderOpen, X } from 'lucide-react'
import { DEPOSTACK_URL } from './constants'
import { shouldIgnoreNavigationShortcut } from './lib/utils'

import useTheme from './hooks/useTheme'
import { usePreferencesContext } from './hooks/PreferencesContext'
import useFileDrop from './hooks/useFileDrop'
import useConversion from './hooks/useConversion'
import useUpdater from './hooks/useUpdater'
import UpdateBanner from './components/UpdateBanner'

import { LogoSvg } from './components/common/Icons'
import WorkspaceLoading from './components/common/WorkspaceLoading'
import { Tabs, TabsList, TabsTrigger, TabsContent } from './components/ui/tabs'
import { Badge } from './components/ui/badge'
import { Button } from './components/ui/button'
import ConvertTab from './components/Convert/ConvertTab'

const LibraryTab = lazy(() => import('./components/Library/LibraryTab'))
const PlayerTab = lazy(() => import('./components/Player/PlayerTab'))
// Merge is hidden for v1 — MergeTab.jsx is retained; re-add the nav entry and
// TabsContent below to bring it back (see the "Re-add Merge" tracking issue).
const SettingsPanel = lazy(() => import('./components/SettingsPanel'))

const themeIcons = {
  light: Sun,
  dark: Moon,
  system: Monitor,
}

// Sidebar navigation: tab id → icon, label, and its number-key shortcut
const NAV = [
  { id: 'convert', label: 'Convert', Icon: AudioLines },
  { id: 'player', label: 'Player', Icon: Play },
  { id: 'library', label: 'Library', Icon: FolderOpen },
]

export default function App() {
  const [tab, setTab] = useState('convert')

  // Custom hooks
  const prefs = usePreferencesContext()
  const { labels, outDir, prefsReady, prefsError } = prefs
  const { themePref, themeLabel, cycleTheme, setThemeDirect } = useTheme(prefs.themePref, prefs.setThemePref)
  const [settingsOpen, setSettingsOpen] = useState(false)

  // Auto-update via GitHub Releases (checks once on launch)
  const updater = useUpdater()

  // While the Player tab is mounted it registers its own drop handler here,
  // so native drops land in the playlist instead of the convert queue
  const dropOverrideRef = useRef(null)
  const conversion = useConversion()
  const { jobs, converting, cancelling, cancelError, doneCount, failCount, cancelledCount } = conversion
  const queueLocked = converting || !prefsReady
  const fileDrop = useFileDrop(dropOverrideRef, queueLocked)
  const {
    files,
    dragOver,
    caseName,
    setCaseName,
    queueNotice,
    addFiles,
    onDragOver,
    onDragLeave,
    onDrop,
    browseFiles,
    browseOutDir,
    removeFile,
    clearAll,
  } = fileDrop

  const [systemError, setSystemError] = useState('')

  // Sidebar health card: bundled audio engine
  const [health, setHealth] = useState(null)
  useEffect(() => {
    invoke('health_check')
      .then(setHealth)
      .catch(error => setSystemError(`Audio engine health could not be checked: ${String(error)}`))
  }, [])

  // Library state
  const [cases, setCases] = useState([])
  const [libSearch, setLibSearch] = useState('')
  const [libraryReadError, setLibraryReadError] = useState('')
  const [libraryOperationError, setLibraryOperationError] = useState('')
  const reportLibraryError = useCallback(error => {
    setLibraryOperationError(`${String(error)} Existing Library data was left unchanged.`)
  }, [])
  const loadLibrary = useCallback(() => {
    invoke('library_get')
      .then(value => {
        setCases(value)
        setLibraryReadError('')
        setLibraryOperationError('')
      })
      .catch(error => {
        setLibraryReadError(
          `Library storage could not be loaded: ${String(error)}. Library changes are blocked to protect the existing data file.`,
        )
      })
  }, [])

  // Load library on startup (nav badge count) and when opening the tab
  useEffect(() => {
    loadLibrary()
  }, [loadLibrary])
  useEffect(() => {
    if (tab === 'library') loadLibrary()
  }, [tab, loadLibrary])

  // Number keys switch tabs (ignored while typing or with modifiers held)
  useEffect(() => {
    const onKey = e => {
      if (shouldIgnoreNavigationShortcut(e)) return
      const idx = ['1', '2', '3', '4'].indexOf(e.key)
      if (idx >= 0 && idx < NAV.length) setTab(NAV[idx].id)
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

  const handleStartConversion = () => {
    if (!prefsReady || files.some(file => file.fmt?.status === 'unsupported')) return
    conversion.startConversion({
      files,
      outDir,
      ...prefs,
      autoLevel: prefs.mode !== 'keep' && prefs.autoLevel,
      denoise: false,
      enhance: false,
      dereverb: false,
      caseName,
      setCases,
      onLibraryError: reportLibraryError,
    })
  }

  const handleQueueFromLibrary = async (sourcePaths, sourceCaseName) => {
    const paths = Array.isArray(sourcePaths) ? sourcePaths : [sourcePaths]
    const result = await addFiles(paths, { replace: true })
    if (result?.locked) return false
    setCaseName(sourceCaseName || '')
    setTab('convert')
    return true
  }

  const ThemeIcon = themeIcons[themeLabel] || Monitor
  const libCount = cases.filter(c => !c.archived).length
  const updaterLabel = updater.error
    ? 'Update check unavailable'
    : updater.status === 'available'
      ? 'Update available'
      : updater.status === 'checking'
        ? 'Checking updates…'
        : updater.status === 'downloading' || updater.status === 'ready'
          ? 'Installing update…'
          : 'Up to date'

  const storageSummary = [prefsError && 'preferences', libraryReadError && 'Library data'].filter(Boolean).join(' and ')
  const systemErrorSummary = systemError.startsWith('Could not open DepoStack')
    ? 'The DepoStack website could not be opened. Check your connection or open depostack.com in a browser.'
    : 'The audio engine check is unavailable. Verify the engine status before starting a conversion.'

  return (
    <Tabs
      value={tab}
      onValueChange={setTab}
      orientation="vertical"
      className="app-shell flex h-full min-h-0 overflow-hidden"
    >
      {/* ── Sidebar ── */}
      <aside
        className="app-sidebar w-14 sm:w-48 shrink-0 flex flex-col bg-card border-r border-border select-none"
        data-tauri-drag-region
      >
        <div className="app-brand flex items-center gap-2.5 px-3 sm:px-4 pt-4 pb-5">
          <LogoSvg />
          <div className="hidden sm:flex flex-col leading-none">
            <span className="text-[16px] font-semibold tracking-[-0.02em] text-foreground">DepoAudio</span>
            <span className="text-[9.5px] text-[hsl(var(--sub))] tracking-wide">Court-audio workbench</span>
          </div>
        </div>

        <TabsList
          aria-label="Main navigation"
          className="app-navigation flex-col items-stretch gap-1 bg-transparent border-none rounded-none p-0 px-2 sm:px-3 h-auto"
        >
          {NAV.map(({ id, label, Icon }, i) => (
            <TabsTrigger
              key={id}
              value={id}
              aria-label={label}
              className="w-full justify-start gap-2.5 px-2.5 sm:px-3 py-2 rounded-lg"
            >
              <Icon size={16} aria-hidden="true" className="shrink-0" />
              <span className="hidden sm:inline">{label}</span>
              {id === 'library' && libCount > 0 && (
                <Badge variant="gold" className="hidden sm:inline-flex">
                  {libCount}
                </Badge>
              )}
              <kbd
                aria-hidden="true"
                className="hidden sm:inline ml-auto font-mono text-[9.5px] px-1.5 py-px rounded border border-border/70 bg-secondary/60 text-[hsl(var(--sub))]"
              >
                {i + 1}
              </kbd>
            </TabsTrigger>
          ))}
        </TabsList>

        <div className="app-sidebar-spacer flex-1" />

        {/* System health */}
        <div
          role="status"
          aria-live="polite"
          aria-atomic="true"
          aria-label="Audio engine and update status"
          className="hidden sm:block mx-3 mb-2 border-y border-border/70 py-2.5"
        >
          <div className="flex items-center gap-2 py-0.5 text-[11px] font-medium text-[hsl(var(--text2))]">
            <span
              aria-hidden="true"
              className={`w-1.5 h-1.5 rounded-full ${health ? (health.ffmpeg ? 'bg-[hsl(var(--success))]' : 'bg-destructive') : 'bg-[hsl(var(--sub))]'}`}
            />
            {health ? (health.ffmpeg ? 'FFmpeg ready' : 'FFmpeg missing') : 'Checking engine…'}
          </div>
          <div className="py-0.5 text-[11px] text-[hsl(var(--sub))]">{updaterLabel}</div>
        </div>

        {/* DepoStack brand link — DepoAudio is one tool in the suite */}
        <button
          type="button"
          onClick={() =>
            openUrl(DEPOSTACK_URL).catch(error => setSystemError(`Could not open DepoStack: ${String(error)}`))
          }
          title="Part of the DepoStack suite — opens depostack.com"
          className="hidden sm:block mx-3 mb-1.5 py-1 text-left text-[10.5px] text-[hsl(var(--sub))] hover:text-foreground transition-colors"
        >
          A <span className="font-semibold text-[hsl(var(--text2))]">DepoStack</span> project&nbsp;↗
        </button>

        <div className="app-sidebar-actions flex sm:justify-start justify-center items-center gap-1 px-2 sm:px-3 pb-3">
          <Button
            variant="ghost"
            size="icon"
            title={prefsError ? 'Settings disabled because preference storage could not be loaded' : 'Settings'}
            aria-label="Settings"
            disabled={!prefsReady || !!prefsError}
            onClick={() => setSettingsOpen(true)}
          >
            <Settings size={16} aria-hidden="true" />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            title={
              prefsError ? 'Theme changes disabled because preference storage is read-only' : `Theme: ${themePref}`
            }
            aria-label={`Switch theme (current: ${themePref})`}
            disabled={!!prefsError}
            onClick={cycleTheme}
          >
            <ThemeIcon size={16} aria-hidden="true" />
          </Button>
        </div>
      </aside>

      {/* ── Main column ── */}
      <main
        className="app-main flex-1 flex flex-col min-w-0 overflow-hidden"
        aria-labelledby="depoaudio-workspace-title"
      >
        <h1 id="depoaudio-workspace-title" className="sr-only">
          DepoAudio workspace
        </h1>
        <UpdateBanner updater={updater} />

        <div className="app-alert-stack shrink-0 overflow-y-auto">
          {(prefsError || libraryReadError) && (
            <div
              role="alert"
              className="mx-5 md:mx-8 mt-3 rounded-lg border border-destructive/40 bg-destructive/10 px-3 py-2 text-[11px] text-foreground"
            >
              <span className="font-semibold">Storage protection active.</span> {storageSummary || 'Storage'} could not
              be loaded. Changes that could overwrite existing data are disabled.
              <details className="mt-1.5">
                <summary className="w-fit cursor-pointer font-semibold text-[hsl(var(--text2))] focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1">
                  Technical details
                </summary>
                <p className="mt-1 whitespace-pre-wrap break-words text-[10px] text-[hsl(var(--sub))]">
                  {[prefsError, libraryReadError].filter(Boolean).join(' ')}
                </p>
              </details>
            </div>
          )}

          {libraryOperationError && (
            <div
              role="alert"
              className="mx-5 md:mx-8 mt-3 rounded-lg border border-warning/40 bg-warning/10 px-3 py-2 text-[11px] text-foreground"
            >
              <span className="font-semibold">Library change not saved.</span> Existing Library data remains unchanged.
              <details className="mt-1.5">
                <summary className="w-fit cursor-pointer font-semibold text-[hsl(var(--text2))] focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1">
                  Technical details
                </summary>
                <p className="mt-1 whitespace-pre-wrap break-words text-[10px] text-[hsl(var(--sub))]">
                  {libraryOperationError}
                </p>
              </details>
            </div>
          )}

          {systemError && (
            <div
              role="alert"
              className="mx-5 md:mx-8 mt-3 flex items-start gap-2 rounded-lg border border-warning/40 bg-warning/10 px-3 py-2 text-[11px] text-foreground"
            >
              <div className="flex-1">
                <span>{systemErrorSummary}</span>
                <details className="mt-1.5">
                  <summary className="w-fit cursor-pointer font-semibold text-[hsl(var(--text2))] focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1">
                    Technical details
                  </summary>
                  <p className="mt-1 whitespace-pre-wrap break-words text-[10px] text-[hsl(var(--sub))]">
                    {systemError}
                  </p>
                </details>
              </div>
              <button
                type="button"
                className="rounded-sm p-1 focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1"
                aria-label="Dismiss system warning"
                onClick={() => setSystemError('')}
              >
                <X aria-hidden="true" className="h-3.5 w-3.5" />
              </button>
            </div>
          )}
        </div>

        {!prefsReady && <WorkspaceLoading label="Loading preferences" />}

        {prefsReady && (
          <>
            <TabsContent value="convert" forceMount={tab === 'convert' ? true : undefined}>
              {tab === 'convert' && (
                <ConvertTab
                  files={files}
                  dragOver={dragOver}
                  caseName={caseName}
                  setCaseName={setCaseName}
                  onDragOver={onDragOver}
                  onDragLeave={onDragLeave}
                  onDrop={onDrop}
                  browseFiles={browseFiles}
                  browseOutDir={browseOutDir}
                  removeFile={removeFile}
                  clearAll={clearAll}
                  jobs={jobs}
                  converting={converting}
                  cancelling={cancelling}
                  conversionError={cancelError}
                  queueLocked={queueLocked}
                  queueNotice={queueNotice}
                  startConversion={handleStartConversion}
                  cancelConversion={conversion.cancelConversion}
                  doneCount={doneCount}
                  failCount={failCount}
                  cancelledCount={cancelledCount}
                />
              )}
            </TabsContent>

            <TabsContent value="player">
              <Suspense fallback={<WorkspaceLoading label="Loading Player workspace" />}>
                <PlayerTab
                  dropHandlerRef={dropOverrideRef}
                  onConvertFiles={async paths => {
                    const result = await addFiles(paths)
                    if (!result?.locked) setTab('convert')
                    return !result?.locked
                  }}
                />
              </Suspense>
            </TabsContent>

            <TabsContent value="library">
              <Suspense fallback={<WorkspaceLoading label="Loading Library workspace" />}>
                <LibraryTab
                  cases={cases}
                  setCases={setCases}
                  search={libSearch}
                  setSearch={setLibSearch}
                  labels={labels}
                  conversionLocked={queueLocked}
                  readOnly={!!libraryReadError}
                  onStorageError={reportLibraryError}
                  onReload={loadLibrary}
                  onReexport={handleQueueFromLibrary}
                />
              </Suspense>
            </TabsContent>
          </>
        )}
      </main>

      <Suspense fallback={null}>
        <SettingsPanel
          open={settingsOpen}
          onOpenChange={setSettingsOpen}
          prefs={{ ...prefs, cycleThemeTo: setThemeDirect }}
          updater={updater}
        />
      </Suspense>
    </Tabs>
  )
}
