import { useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { MODES, FORMATS_OUT, CH_COLORS } from '../../constants'
import { PRESETS } from '../../presets'
import { Loader2 } from 'lucide-react'
import { Button } from '../ui/button'
import { Card, CardHeader, CardTitle, CardContent } from '../ui/card'
import { Badge } from '../ui/badge'
import { Input } from '../ui/input'
import { Label } from '../ui/label'
import { ModeIcon, WaveformIcon } from '../common/Icons'
import ProcessingToggle from './ProcessingToggle'
import FormatTable from './FormatTable'
import FileRow from './FileRow'

export default function ConvertTab({
  // Preferences
  mode, setMode, formatOut, setFormatOut, labels, setLabels,
  chanVols, setChanVols, outDir, setOutDir, rate, setRate,
  normalize, setNormalize, trim, setTrim, fade, setFade,
  fadeDur, setFadeDur, hpf, setHpf,
  denoise, setDenoise, denoiseQuality, setDenoiseQuality,
  autoLevel, setAutoLevel, declip, setDeclip, enhance, setEnhance,
  dereverb, setDereverb, capabilities,
  // Files
  files, dragOver, caseName, setCaseName,
  onDragOver, onDragLeave, onDrop, browseFiles, browseOutDir,
  removeFile, clearAll,
  // Conversion
  jobs, converting, startConversion, doneCount, failCount,
}) {
  const anyProc = normalize || trim || fade || hpf
  const anyAi = denoise || autoLevel || declip || enhance || dereverb
  const [analysis, setAnalysis] = useState(null)
  const [scanning, setScanning] = useState(false)

  const handleScan = async () => {
    if (!files.length || scanning) return
    setScanning(true)
    try {
      // Scan all files and aggregate results
      const results = []
      for (const file of files) {
        try {
          const result = await invoke('analyze_audio_cmd', { path: file.path })
          results.push(result)
        } catch (e) {
          // Skip files that fail analysis
        }
      }

      if (results.length > 0) {
        // Aggregate: use worst-case across all files
        const aggregated = {
          ...results[0],
          needsDenoise: results.some(r => r.needsDenoise),
          needsLeveling: results.some(r => r.needsLeveling),
          hasClipping: results.some(r => r.hasClipping),
          isNarrowband: results.some(r => r.isNarrowband),
          recommendations: [...new Set(results.flatMap(r => r.recommendations || []))],
          qualityScore: results[0].qualityScore,
          speakerCount: Math.max(...results.map(r => r.speakerCount || 0)) || null,
          speechRatio: results.reduce((sum, r) => sum + (r.speechRatio || 0), 0) / results.length,
        }
        setAnalysis(aggregated)

        // Auto-enable all recommended processing
        if (aggregated.needsDenoise) setDenoise(true)
        if (aggregated.needsLeveling) setAutoLevel(true)
        if (aggregated.hasClipping) setDeclip(true)
        if (aggregated.isNarrowband) setEnhance(true)
        // Smart fine-tune: always enable HPF + normalize for speech
        setHpf(true)
        setNormalize(true)
        // Trim silence if there's significant dead air
        if (aggregated.speechRatio != null && aggregated.speechRatio < 0.8) setTrim(true)
        // Fade for clean start/end
        setFade(true)
      }
    } catch (e) {
      console.error('Scan failed:', e)
    }
    setScanning(false)
  }

  return (
    <>
      <div className="flex-1 overflow-y-auto overflow-x-hidden scrollbar-thin scrollbar-thumb-border scrollbar-track-transparent">
        <div className="max-w-[920px] mx-auto px-7 py-5 flex flex-col gap-3.5">

          {/* ── PRESETS ──────────────────────────────────────────────────── */}
          <div className="flex items-center gap-1.5 flex-wrap mb-2">
            <Label>PRESET</Label>
            {PRESETS.map(p => (
              <Button key={p.id} variant="outline" size="sm" className="rounded-full" title={p.desc}
                onClick={() => {
                  const s = p.settings
                  setMode(s.mode); setFormatOut(s.format); setRate(s.rate)
                  setNormalize(s.normalize); setTrim(s.trim); setFade(s.fade)
                  setFadeDur(s.fadeDur); setHpf(s.hpf)
                  setDenoise(s.denoise); setDenoiseQuality(s.denoiseQuality)
                  setAutoLevel(s.autoLevel); setDeclip(s.declip)
                  setEnhance(s.enhance); setDereverb(s.dereverb)
                }}>
                {p.name}
              </Button>
            ))}
          </div>

          {/* ── OUTPUT MODE ──────────────────────────────────────────────── */}
          <Card>
            <CardHeader><CardTitle>OUTPUT MODE</CardTitle></CardHeader>
            <CardContent>
              <div className="grid grid-cols-3 gap-2 p-3">
                {MODES.map(m => (
                  <button key={m.id}
                    className={`flex items-center gap-3 p-2.5 rounded-lg border transition-colors cursor-pointer ${mode===m.id ? 'border-primary bg-[hsl(var(--gold-dim))]' : 'border-transparent hover:bg-secondary/50'}`}
                    onClick={() => setMode(m.id)}>
                    <ModeIcon id={m.id} active={mode===m.id} />
                    <div className="flex flex-col items-start">
                      <span className="text-[12px] font-semibold text-foreground">{m.label}</span>
                      <span className="text-[10px] text-[hsl(var(--sub))] leading-tight">{m.desc}</span>
                    </div>
                    {mode===m.id && <span className="ml-auto text-primary text-sm">✓</span>}
                  </button>
                ))}
              </div>
            </CardContent>
          </Card>

          {/* ── CHANNEL LABELS ──────────────────────────────────────────── */}
          <Card>
            <CardHeader>
              <CardTitle>CHANNEL LABELS</CardTitle>
              <span className="text-[10px] text-[hsl(var(--sub))]">
                {mode === 'split'  && 'Used as output filenames when splitting channels'}
                {mode === 'stereo' && 'Labels shown in mix sliders below'}
                {mode === 'keep'   && 'Labels saved to library for reference'}
              </span>
            </CardHeader>
            <CardContent>
              <div className="grid grid-cols-4 gap-2 p-3">
                {labels.map((l,i) => (
                  <div key={i} className="flex items-center gap-1.5">
                    <span className="w-2.5 h-2.5 rounded-full shrink-0" style={{background:CH_COLORS[i]}} />
                    <span className="font-mono text-[10px] text-[hsl(var(--sub))] shrink-0">CH {i+1}</span>
                    <Input className="h-7 text-[11px]" value={l} maxLength={24} placeholder={`Channel ${i+1}`}
                      onChange={e => setLabels(p => p.map((v,j) => j===i ? e.target.value:v))} />
                  </div>
                ))}
              </div>
            </CardContent>
          </Card>

          {/* ── CHANNEL MIX ─────────────────────────────────────────────── */}
          <Card className={mode !== 'stereo' || autoLevel ? 'opacity-50' : ''}>
            <CardHeader>
              <CardTitle>CHANNEL MIX</CardTitle>
              {autoLevel
                ? <span className="text-[10px] text-[hsl(var(--sub))] italic">Managed by Balance Speaker Volume above</span>
                : mode !== 'stereo'
                  ? <span className="text-[10px] text-[hsl(var(--sub))] italic">Active in Mix to Stereo mode</span>
                  : <span className="text-[10px] text-[hsl(var(--sub))]">Per-channel volume — 1.0 = unity, 0.0 = mute, 2.0 = boost</span>
              }
            </CardHeader>
            <CardContent>
              <div className="flex flex-col gap-1.5 p-3">
                {chanVols.map((v,i) => (
                  <div key={i} className={`flex items-center gap-2 ${mode !== 'stereo' || autoLevel ? 'opacity-50' : ''}`}>
                    <span className="w-2 h-2 rounded-full shrink-0" style={{background:CH_COLORS[i]}} />
                    <span className="text-[11px] text-[hsl(var(--text2))] w-24 truncate">{labels[i]||`CH ${i+1}`}</span>
                    <input type="range" min="0" max="2" step="0.05" value={autoLevel ? 1.0 : v}
                      className="flex-1 h-1 accent-primary cursor-pointer"
                      style={{'--fill':`${((autoLevel ? 1.0 : v)/2)*100}%`}}
                      disabled={mode !== 'stereo' || autoLevel}
                      onChange={e => setChanVols(p => p.map((x,j) => j===i ? parseFloat(e.target.value):x))} />
                    <span className="font-mono text-[10px] text-[hsl(var(--sub))] w-8 text-right">{autoLevel ? 'auto' : v===0?'mute':v.toFixed(2)}</span>
                  </div>
                ))}
              </div>
            </CardContent>
          </Card>

          {/* ── AUDIO PROCESSING (unified panel) ─────────────────────────── */}
          <Card>
            <CardHeader>
              <CardTitle>AUDIO PROCESSING</CardTitle>
              <Button variant="scan" size="sm" onClick={handleScan}
                disabled={!files.length || scanning || converting}>
                {scanning ? `Scanning${files.length > 1 ? ` (${files.length} files)` : ''}…` : `Scan${files.length > 1 ? ` All (${files.length})` : ''}`}
              </Button>
            </CardHeader>
            <CardContent>
              {!analysis && !scanning && (
                <p className="px-4 py-2.5 text-[11px] text-[hsl(var(--sub))]">Drop files and click <strong className="text-foreground">Scan</strong> to detect issues and auto-enable the right fixes.</p>
              )}

              {scanning && (
                <p className="px-4 py-2.5 text-[11px] text-[hsl(var(--sub))] flex items-center gap-1.5">
                  <Loader2 className="animate-spin h-3.5 w-3.5" /> Listening to your audio…
                </p>
              )}

              {analysis && (
                <div className="flex flex-wrap gap-1.5 px-4 py-2.5">
                  {analysis.qualityScore && (
                    <Badge variant="outline" title={`Signal: ${analysis.qualityScore.sig?.toFixed(1)} · Background: ${analysis.qualityScore.bak?.toFixed(1)}`}>
                      Quality: {analysis.qualityScore.ovr?.toFixed(1)}/5
                    </Badge>
                  )}
                  {analysis.speakerCount != null && (
                    <Badge variant="outline">
                      {analysis.speakerCount} speaker{analysis.speakerCount !== 1 ? 's' : ''} detected
                    </Badge>
                  )}
                  {analysis.turns?.length > 0 && (
                    <Badge variant="outline">
                      {analysis.turns.length} turn{analysis.turns.length !== 1 ? 's' : ''} found
                    </Badge>
                  )}
                  {analysis.speechRatio != null && (
                    <Badge variant="outline">
                      {Math.round(analysis.speechRatio * 100)}% speech
                    </Badge>
                  )}
                </div>
              )}

              {/* ── Smart (auto-detected) ── */}
              <div className="px-4 py-1.5 bg-secondary/50">
                <span className="font-mono text-[9px] font-medium tracking-[1.2px] uppercase text-[hsl(var(--sub))]">SMART</span>
              </div>

              <ProcessingToggle smart name="Remove Background Noise"
                desc="Cleans up HVAC hum, paper rustling, and room noise"
                checked={denoise} onChange={setDenoise}
                detected={analysis?.needsDenoise ? 'Noise detected' : null}
                extra={denoise && (
                  <span className="inline-flex items-center gap-1 mt-0.5" onClick={e => e.preventDefault()}>
                    <span className="text-[hsl(var(--sub))]">—</span>
                    <select className="font-mono text-[10px] bg-secondary border border-border rounded px-1 py-px text-[hsl(var(--text2))] cursor-pointer"
                      value={denoiseQuality} onChange={e => { e.stopPropagation(); setDenoiseQuality(e.target.value) }}>
                      <option value="fast">Fast</option>
                      <option value="best">Best quality</option>
                    </select>
                  </span>
                )}
              />

              <ProcessingToggle smart name="Balance Speaker Volume"
                desc="Evens out volume so quiet speakers are easier to hear"
                checked={autoLevel} onChange={v => { setAutoLevel(v); }}
                detected={analysis?.needsLeveling ? `${analysis.recommendations?.find(r => r.includes('spread'))?.match(/[\d.]+/)?.[0] || ''}dB imbalance found` : null}
              />

              <ProcessingToggle smart name="Fix Clipped Audio"
                desc="Repairs distorted peaks from recordings that were too loud"
                checked={declip} onChange={setDeclip}
                detected={analysis?.hasClipping ? 'Clipping found' : null}
              />

              <ProcessingToggle smart name="Enhance Clarity"
                desc="Improves phone recordings and narrow-band audio"
                checked={enhance} onChange={setEnhance}
                detected={analysis?.isNarrowband ? `${analysis.sampleRate?.toLocaleString() || ''}Hz detected` : null}
              />

              <ProcessingToggle smart name="Reduce Room Echo"
                desc="Removes reverb from large rooms or hallways"
                checked={dereverb} onChange={setDereverb}
              />

              {/* ── Fine-tune ── */}
              <div className="px-4 py-1.5 bg-secondary/50">
                <span className="font-mono text-[9px] font-medium tracking-[1.2px] uppercase text-[hsl(var(--sub))]">FINE-TUNE</span>
              </div>

              <ProcessingToggle name="High-Pass Filter"
                desc="80 Hz cutoff — removes low rumble and handling noise"
                checked={hpf} onChange={setHpf}
              />

              <ProcessingToggle name="Normalize Volume"
                desc="Targets –16 LUFS / –1.5 TP for consistent output level"
                checked={normalize} onChange={setNormalize}
              />

              <ProcessingToggle name="Trim Silence"
                desc="Remove dead air at start and end (below –50 dB)"
                checked={trim} onChange={setTrim}
              />

              <ProcessingToggle name="Fade In / Out"
                desc="Smooth start and end"
                checked={fade} onChange={setFade}
                extra={fade && (
                  <span className="inline-flex items-center gap-1 mt-0.5" onClick={e => e.preventDefault()}>
                    <span className="text-[hsl(var(--sub))]">—</span>
                    <input type="number" className="w-[42px] bg-secondary border border-border rounded px-1 py-px font-mono text-[11px] text-foreground text-center focus:border-primary outline-none"
                      min="0.1" max="5" step="0.1" value={fadeDur}
                      onChange={e => setFadeDur(Math.max(0.1, parseFloat(e.target.value)||0.5))} />
                    <span className="text-[11px] text-[hsl(var(--sub))]">s</span>
                  </span>
                )}
              />

              {/* Processing chain preview */}
              <div className="flex items-center gap-2 px-4 py-2.5 border-t border-border/60 bg-secondary/30">
                <span className="font-mono text-[9px] font-medium tracking-[1.2px] uppercase text-[hsl(var(--sub))] shrink-0">CHAIN</span>
                {(anyProc || anyAi) ? (
                  <div className="flex items-center gap-1 flex-wrap">
                    {denoise   && <><Badge variant="info">Denoise</Badge><span className="text-[10px] text-[hsl(var(--sub))]">→</span></>}
                    {dereverb  && <><Badge variant="info">De-reverb</Badge><span className="text-[10px] text-[hsl(var(--sub))]">→</span></>}
                    {enhance   && <><Badge variant="info">Enhance</Badge><span className="text-[10px] text-[hsl(var(--sub))]">→</span></>}
                    {declip    && <><Badge variant="info">De-clip</Badge><span className="text-[10px] text-[hsl(var(--sub))]">→</span></>}
                    {hpf       && <><Badge variant="default">HPF</Badge><span className="text-[10px] text-[hsl(var(--sub))]">→</span></>}
                    {autoLevel && <><Badge variant="info">Auto-Level</Badge><span className="text-[10px] text-[hsl(var(--sub))]">→</span></>}
                    {normalize && <><Badge variant="default">Normalize</Badge><span className="text-[10px] text-[hsl(var(--sub))]">→</span></>}
                    {trim      && <><Badge variant="default">Trim</Badge><span className="text-[10px] text-[hsl(var(--sub))]">→</span></>}
                    {fade      && <Badge variant="default">Fade {fadeDur}s</Badge>}
                  </div>
                ) : (
                  <span className="text-[10px] text-[hsl(var(--sub))] italic">No processing — direct transcode only</span>
                )}
              </div>

              <p className="px-4 py-2 text-[10px] text-[hsl(var(--sub))] border-t border-border/60">
                All processing runs on your machine — nothing is uploaded or sent anywhere.
                {capabilities && (
                  <span className="opacity-60" title={`${capabilities.cpuCores} cores · ${Math.round(capabilities.ramMb/1024)}GB RAM${capabilities.appleSilicon ? ' · Apple Silicon' : ''}`}>
                    {' '}· {capabilities.tier === 'high' ? 'High performance' : capabilities.tier === 'mid' ? 'Standard performance' : 'Lightweight mode'}
                  </span>
                )}
              </p>
            </CardContent>
          </Card>

          {/* ── OPTIONS ──────────────────────────────────────────────────── */}
          <div className="flex items-end gap-3 flex-wrap">
            <div className="flex-1 min-w-[200px]">
              <Label className="mb-1 block">CASE NAME</Label>
              <Input value={caseName} placeholder="Auto-detected from filename — override here"
                onChange={e => setCaseName(e.target.value)} />
            </div>
            <div className="flex-1 min-w-[200px]">
              <Label className="mb-1 block">OUTPUT FOLDER</Label>
              <div className="flex gap-1.5">
                <Input className="flex-1" value={outDir} placeholder="Default: same folder as source"
                  onChange={e => setOutDir(e.target.value)} />
                <Button variant="default" size="sm" onClick={() => browseOutDir(setOutDir)}>Browse</Button>
              </div>
            </div>
            <div className="min-w-[130px]">
              <Label className="mb-1 block">SAMPLE RATE</Label>
              <select className="flex h-8 w-full rounded-lg border border-border bg-card px-2.5 py-1.5 text-xs text-foreground transition-colors focus:outline-none focus:border-primary cursor-pointer"
                value={formatOut === 'opus' ? '48000' : rate}
                disabled={formatOut === 'opus'}
                title={formatOut === 'opus' ? 'Opus is always 48 kHz' : ''}
                onChange={e => setRate(e.target.value)}>
                <option value="22050">22,050 Hz</option>
                <option value="44100">44,100 Hz</option>
                <option value="48000">48,000 Hz</option>
              </select>
            </div>
            <div>
              <Label className="mb-1 block">OUTPUT FORMAT</Label>
              <div className="flex gap-px bg-secondary rounded-md p-0.5">
                {FORMATS_OUT.map(f => (
                  <button key={f.id} title={f.desc}
                    className={`px-2.5 py-1.5 text-[11px] font-semibold rounded-md transition-colors cursor-pointer ${formatOut===f.id ? 'bg-card text-foreground shadow-sm' : 'text-[hsl(var(--sub))] hover:text-[hsl(var(--text2))]'}`}
                    onClick={() => setFormatOut(f.id)}>{f.label}</button>
                ))}
              </div>
            </div>
          </div>

          <FormatTable />

          {/* Drop zone */}
          <div className={`flex flex-col items-center justify-center gap-2 py-8 px-6 border-2 border-dashed rounded-xl cursor-pointer transition-colors ${dragOver ? 'border-primary bg-[hsl(var(--gold-dim))]' : 'border-border/60 hover:border-border hover:bg-secondary/30'}`}
            onDragOver={onDragOver} onDragLeave={onDragLeave} onDrop={onDrop}
            onClick={browseFiles}>
            <WaveformIcon />
            <p className="text-[13px] font-semibold text-foreground">Drop audio files here</p>
            <p className="text-[11px] text-[hsl(var(--sub))] text-center">or <span className="text-primary cursor-pointer hover:underline">click to browse</span> — WAV · MP3 · FLAC · Opus · M4A · OGG · SGMCA · TRM · BWF and more</p>
          </div>

          {files.length > 0 && (
            <div className="flex flex-col gap-2">
              <div className="flex items-center justify-between">
                <span className="font-mono text-[11px] text-[hsl(var(--sub))]">{files.length} file{files.length!==1?'s':''} queued</span>
                {!converting && <button className="text-[11px] text-[hsl(var(--sub))] hover:text-foreground transition-colors cursor-pointer" onClick={() => clearAll(converting)}>Clear all</button>}
              </div>
              <div className="flex flex-col gap-1.5">
                {files.map(f => <FileRow key={f.path} file={f} job={jobs[f.path]} onRemove={() => removeFile(f.path, converting)} converting={converting} />)}
              </div>
            </div>
          )}

        </div>
      </div>

      <footer className="flex items-center justify-between px-7 py-3 border-t border-border/60 bg-card shrink-0">
        <div className="flex items-center gap-2">
          {converting && (
            <Badge variant="active">
              <span className="w-1.5 h-1.5 rounded-full bg-primary animate-pulse mr-1.5" />
              {doneCount > 0 ? `${doneCount} / ${files.length} done` : 'Converting…'}
            </Badge>
          )}
          {!converting && doneCount > 0 && (
            <Badge variant="done">
              ✓ {doneCount} file{doneCount!==1?'s':''} converted{failCount>0?`, ${failCount} failed`:''}
            </Badge>
          )}
        </div>
        <Button variant="primary" size="lg"
          onClick={startConversion} disabled={converting||!files.length}>
          {converting ? <><Loader2 className="animate-spin h-3.5 w-3.5" />Converting…</> : <>▶ Convert{files.length > 1 ? ` ${files.length} Files` : ''}</>}
        </Button>
      </footer>
    </>
  )
}
