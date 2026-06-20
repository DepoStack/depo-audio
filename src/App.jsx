import { useState, useEffect, useRef, lazy, Suspense } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { Sun, Moon, Monitor, Settings } from 'lucide-react'
import { basename } from './utils'

import useTheme from './hooks/useTheme'
import { usePreferencesContext } from './hooks/PreferencesContext'
import useFileDrop from './hooks/useFileDrop'
import useConversion from './hooks/useConversion'

import { LogoSvg } from './components/common/Icons'
import Spinner from './components/common/Spinner'
import { Tabs, TabsList, TabsTrigger, TabsContent } from './components/ui/tabs'
import { Badge } from './components/ui/badge'
import { Button } from './components/ui/button'
import ConvertTab from './components/Convert/ConvertTab'

const LibraryTab = lazy(() => import('./components/Library/LibraryTab'))
const PlayerTab  = lazy(() => import('./components/Player/PlayerTab'))
const MergeTab   = lazy(() => import('./components/Merge/MergeTab'))
const SettingsPanel = lazy(() => import('./components/SettingsPanel'))

const themeIcons = {
  light: Sun,
  dark: Moon,
  system: Monitor,
}

export default function App() {
  const [tab, setTab] = useState('convert')

  // Custom hooks
  const { themePref, themeLabel, cycleTheme, setThemeDirect } = useTheme()
  const [settingsOpen, setSettingsOpen] = useState(false)

  const prefs = usePreferencesContext()
  const { labels, outDir } = prefs

  // While the Player tab is mounted it registers its own drop handler here,
  // so native drops land in the playlist instead of the convert queue
  const dropOverrideRef = useRef(null)
  const fileDrop = useFileDrop(dropOverrideRef)
  const {
    files, setFiles, dragOver, caseName, setCaseName,
    onDragOver, onDragLeave, onDrop, browseFiles, browseOutDir,
    removeFile, clearAll,
  } = fileDrop

  const conversion = useConversion()
  const { jobs, converting, doneCount, failCount } = conversion

  // System capabilities (hardware-aware recommendations)
  const [capabilities, setCapabilities] = useState(null)
  useEffect(() => {
    invoke('system_capabilities_cmd').then(setCapabilities).catch(() => {})
  }, [])

  // Library state
  const [cases, setCases]     = useState([])
  const [libSearch, setLibSearch] = useState('')

  // Load library on startup (header badge count) and when opening the tab
  useEffect(() => {
    invoke('library_get').then(setCases).catch(() => {})
  }, [])
  useEffect(() => {
    if (tab === 'library') {
      invoke('library_get').then(setCases).catch(() => {})
    }
  }, [tab])

  const handleStartConversion = () => {
    conversion.startConversion({
      files, outDir, ...prefs,
      caseName, setCases,
    })
  }

  const ThemeIcon = themeIcons[themeLabel] || Monitor
  const libCount = cases.filter(c => !c.archived).length

  return (
    <Tabs value={tab} onValueChange={setTab} className="flex flex-col h-screen overflow-hidden">
      {/* ── Topbar */}
      <header
        className="h-(--topbar-h) shrink-0 bg-[hsl(var(--surface))] border-b border-border grid grid-cols-[1fr_auto_1fr] items-center px-5 select-none"
        data-tauri-drag-region
      >
        <div className="flex items-center gap-2.5">
          <LogoSvg />
          <div className="flex flex-col leading-none">
            <span className="font-serif text-[17px] font-semibold text-gold-hi">DepoAudio</span>
            <span className="text-[10px] text-[hsl(var(--sub))] tracking-wider">Audio Converter &amp; Enhancer</span>
          </div>
        </div>
        <TabsList aria-label="Main navigation">
          <TabsTrigger value="convert">Convert</TabsTrigger>
          <TabsTrigger value="player">Player</TabsTrigger>
          <TabsTrigger value="merge">Merge</TabsTrigger>
          <TabsTrigger value="library">
            Library {libCount > 0 && <Badge variant="gold">{libCount}</Badge>}
          </TabsTrigger>
        </TabsList>
        <div className="flex justify-end gap-1">
          <Button variant="ghost" size="icon" title="Settings" aria-label="Settings" onClick={() => setSettingsOpen(true)}>
            <Settings size={16} aria-hidden="true" />
          </Button>
          <Button variant="ghost" size="icon" title={`Theme: ${themePref}`} aria-label={`Switch theme (current: ${themePref})`} onClick={cycleTheme}>
            <ThemeIcon size={16} aria-hidden="true" />
          </Button>
        </div>
      </header>

      <TabsContent value="convert" forceMount={tab === 'convert' ? true : undefined}>
        {tab === 'convert' && (
          <ConvertTab
            capabilities={capabilities}
            files={files} dragOver={dragOver}
            caseName={caseName} setCaseName={setCaseName}
            onDragOver={onDragOver} onDragLeave={onDragLeave} onDrop={onDrop}
            browseFiles={browseFiles} browseOutDir={browseOutDir}
            removeFile={removeFile} clearAll={clearAll}
            jobs={jobs} converting={converting}
            startConversion={handleStartConversion}
            doneCount={doneCount} failCount={failCount}
          />
        )}
      </TabsContent>

      <TabsContent value="player">
        <Suspense fallback={<div className="flex items-center justify-center py-12"><Spinner className="h-5 w-5" /></div>}>
          <PlayerTab dropHandlerRef={dropOverrideRef} />
        </Suspense>
      </TabsContent>

      <TabsContent value="merge">
        <Suspense fallback={<div className="flex items-center justify-center py-12"><Spinner className="h-5 w-5" /></div>}>
          <MergeTab />
        </Suspense>
      </TabsContent>

      <TabsContent value="library">
        <Suspense fallback={<div className="flex items-center justify-center py-12"><Spinner className="h-5 w-5" /></div>}>
          <LibraryTab
            cases={cases} setCases={setCases}
            search={libSearch} setSearch={setLibSearch}
            labels={labels}
            onReexport={(srcPath, srcCaseName) => {
              setFiles([{path:srcPath, name:basename(srcPath), fmt:null}])
              setCaseName(srcCaseName || '')
              setTab('convert')
            }}
          />
        </Suspense>
      </TabsContent>

      <Suspense fallback={null}>
        <SettingsPanel
          open={settingsOpen}
          onOpenChange={setSettingsOpen}
          prefs={{ ...prefs, themePref, cycleThemeTo: setThemeDirect }}
        />
      </Suspense>
    </Tabs>
  )
}
