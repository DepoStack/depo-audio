import { useState, useEffect } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { Sun, Moon, Monitor, Settings } from 'lucide-react'
import { basename } from './utils'

import useTheme from './hooks/useTheme'
import usePreferences from './hooks/usePreferences'
import useFileDrop from './hooks/useFileDrop'
import useConversion from './hooks/useConversion'

import { LogoSvg } from './components/common/Icons'
import { Tabs, TabsList, TabsTrigger, TabsContent } from './components/ui/tabs'
import { Badge } from './components/ui/badge'
import { Button } from './components/ui/button'
import ConvertTab from './components/Convert/ConvertTab'
import LibraryTab from './components/Library/LibraryTab'
import PlayerTab from './components/Player/PlayerTab'
import MergeTab from './components/Merge/MergeTab'
import SettingsPanel from './components/SettingsPanel'

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

  const prefs = usePreferences()
  const {
    mode, setMode, formatOut, setFormatOut, labels, setLabels,
    chanVols, setChanVols, outDir, setOutDir, rate, setRate,
    normalize, setNormalize, trim, setTrim, fade, setFade,
    fadeDur, setFadeDur, hpf, setHpf,
    denoise, setDenoise, denoiseQuality, setDenoiseQuality,
    autoLevel, setAutoLevel, declip, setDeclip, enhance, setEnhance,
    dereverb, setDereverb,
  } = prefs

  const fileDrop = useFileDrop()
  const {
    files, setFiles, dragOver, caseName, setCaseName,
    onDragOver, onDragLeave, onDrop, browseFiles, browseOutDir,
    removeFile, clearAll,
  } = fileDrop

  const conversion = useConversion()
  const { jobs, setJobs, converting, doneCount, failCount } = conversion

  // System capabilities (hardware-aware recommendations)
  const [capabilities, setCapabilities] = useState(null)
  useEffect(() => {
    invoke('system_capabilities_cmd').then(setCapabilities).catch(() => {})
  }, [])

  // Library state
  const [cases, setCases]     = useState([])
  const [libSearch, setLibSearch] = useState('')

  // Load library when switching to library tab
  useEffect(() => {
    if (tab === 'library') {
      invoke('library_get').then(setCases).catch(() => {})
    }
  }, [tab])

  const handleStartConversion = () => {
    conversion.startConversion({
      files, outDir, mode, formatOut, rate,
      labels, chanVols, normalize, trim, fade, fadeDur, hpf,
      denoise, denoiseQuality, autoLevel, declip, enhance, dereverb,
      caseName, setCases,
    })
  }

  const ThemeIcon = themeIcons[themeLabel] || Monitor
  const libCount = cases.filter(c => !c.archived).length

  return (
    <Tabs value={tab} onValueChange={setTab} className="flex flex-col h-screen overflow-hidden">
      {/* ── Topbar */}
      <header
        className="h-[var(--topbar-h)] shrink-0 bg-[hsl(var(--surface))] border-b border-border grid grid-cols-[1fr_auto_1fr] items-center px-5 select-none"
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
          <Button variant="ghost" size="icon" title="Settings" onClick={() => setSettingsOpen(true)}>
            <Settings size={16} />
          </Button>
          <Button variant="ghost" size="icon" title={`Theme: ${themePref}`} onClick={cycleTheme}>
            <ThemeIcon size={16} />
          </Button>
        </div>
      </header>

      <TabsContent value="convert" forceMount={tab === 'convert' ? true : undefined}>
        {tab === 'convert' && (
          <ConvertTab
            mode={mode} setMode={setMode}
            formatOut={formatOut} setFormatOut={setFormatOut}
            labels={labels} setLabels={setLabels}
            chanVols={chanVols} setChanVols={setChanVols}
            outDir={outDir} setOutDir={setOutDir}
            rate={rate} setRate={setRate}
            normalize={normalize} setNormalize={setNormalize}
            trim={trim} setTrim={setTrim}
            fade={fade} setFade={setFade}
            fadeDur={fadeDur} setFadeDur={setFadeDur}
            hpf={hpf} setHpf={setHpf}
            denoise={denoise} setDenoise={setDenoise}
            denoiseQuality={denoiseQuality} setDenoiseQuality={setDenoiseQuality}
            autoLevel={autoLevel} setAutoLevel={setAutoLevel}
            declip={declip} setDeclip={setDeclip}
            enhance={enhance} setEnhance={setEnhance}
            dereverb={dereverb} setDereverb={setDereverb}
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
        <PlayerTab />
      </TabsContent>

      <TabsContent value="merge">
        <MergeTab />
      </TabsContent>

      <TabsContent value="library">
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
      </TabsContent>

      <SettingsPanel
        open={settingsOpen}
        onOpenChange={setSettingsOpen}
        prefs={{ ...prefs, themePref, cycleThemeTo: setThemeDirect }}
      />
    </Tabs>
  )
}
