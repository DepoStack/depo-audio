import { useId } from 'react'
import { Volume2 } from 'lucide-react'
import { cn } from '../../lib/utils'
import {
  CHANNEL_VOLUME_MAX,
  LISTENING_ONLY_NOTE,
  MASTER_VOLUME_STEP,
  audibleChannels,
  separationNote,
  stripLabels,
  volumePercent,
} from '../../lib/channelMixer'

const toggleClass = pressed =>
  cn(
    'h-7 min-w-[3.25rem] rounded-md border px-2 type-small transition-colors',
    pressed
      ? 'border-foreground bg-secondary text-foreground ring-1 ring-inset ring-foreground font-semibold'
      : 'border-border text-[hsl(var(--text2))] hover:text-foreground hover:bg-secondary',
  )

// Master volume for the Player plus, for a file with 2 or more channels, an
// opt-in strip per channel (Mute, Solo, volume). Everything here changes what
// the person hears only.
export default function ListeningMix({
  volume,
  onVolume,
  fileChannels,
  mixOn,
  onToggleMix,
  strips,
  onStrips,
  heardChannels,
  probeAvailable,
  notice = '',
}) {
  const noteId = useId()
  const separateNoteId = useId()
  const labels = stripLabels({ fileChannels, heardChannels })
  const shown = strips.slice(0, labels.length)
  const audible = audibleChannels(shown)
  const anySolo = shown.some(s => s.soloed)
  const nothingAudible = shown.length > 0 && audible.every(a => !a)
  const note = mixOn ? separationNote({ fileChannels, heardChannels, probeAvailable }) : ''
  const update = (i, patch) => onStrips(strips.map((s, j) => (j === i ? { ...s, ...patch } : s)))

  return (
    <section aria-label="Listening mix" className="flex flex-col gap-2 border-t border-border pt-3">
      <p id={noteId} className="type-small text-[hsl(var(--sub))]">
        {LISTENING_ONLY_NOTE}
      </p>
      <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
        <label className="flex min-w-[12rem] flex-1 items-center gap-2 type-small text-[hsl(var(--text2))]">
          <Volume2 size={14} aria-hidden="true" className="shrink-0" />
          <span className="shrink-0">Volume</span>
          <input
            type="range"
            min="0"
            max="1"
            step={MASTER_VOLUME_STEP}
            value={volume}
            aria-label="Volume"
            aria-describedby={noteId}
            aria-valuetext={volumePercent(volume)}
            className="h-1 min-w-0 flex-1 cursor-pointer accent-foreground"
            onChange={e => onVolume(Number(e.target.value))}
          />
          <span className="w-10 shrink-0 text-right tabular-nums text-foreground">{volumePercent(volume)}</span>
        </label>
        {fileChannels >= 2 && (
          <button
            type="button"
            aria-pressed={mixOn}
            aria-describedby={separateNoteId}
            className={toggleClass(mixOn)}
            title={`Hear each of the ${fileChannels} channels on its own. ${LISTENING_ONLY_NOTE}`}
            onClick={onToggleMix}
          >
            Separate channels
          </button>
        )}
        <span id={separateNoteId} className="sr-only">
          {`Mute, solo or turn down each of this file's ${fileChannels || 0} channels. ${LISTENING_ONLY_NOTE}`}
        </span>
      </div>

      {mixOn && shown.length > 0 && (
        <div role="group" aria-label="Channel strips" className="flex flex-col gap-1">
          {shown.map((strip, i) => {
            const label = labels[i]
            return (
              <div
                key={label}
                className="flex flex-wrap items-center gap-2 rounded-md px-1 py-1 sm:flex-nowrap"
                data-testid="channel-strip"
              >
                <span className="w-20 shrink-0 type-small font-semibold text-foreground">{label}</span>
                <button
                  type="button"
                  aria-pressed={strip.muted}
                  aria-label={`Mute ${label}`}
                  className={toggleClass(strip.muted)}
                  onClick={() => update(i, { muted: !strip.muted })}
                >
                  Mute
                </button>
                <button
                  type="button"
                  aria-pressed={strip.soloed}
                  aria-label={`Solo ${label}`}
                  className={toggleClass(strip.soloed)}
                  onClick={() => update(i, { soloed: !strip.soloed })}
                >
                  Solo
                </button>
                <input
                  type="range"
                  min="0"
                  max={CHANNEL_VOLUME_MAX}
                  step="0.05"
                  value={strip.volume}
                  aria-label={`${label} volume`}
                  aria-valuetext={volumePercent(strip.volume)}
                  className="h-1 min-w-[6rem] flex-1 cursor-pointer accent-foreground"
                  onChange={e => update(i, { volume: Number(e.target.value) })}
                />
                <span className="w-10 shrink-0 text-right type-small tabular-nums text-foreground">
                  {volumePercent(strip.volume)}
                </span>
                <span className="w-24 shrink-0 type-small text-[hsl(var(--sub))]">
                  {audible[i] ? '' : strip.muted ? 'Muted' : anySolo ? 'Silent (solo)' : ''}
                </span>
              </div>
            )
          })}
          <p role="status" aria-live="polite" className="type-small text-foreground">
            {nothingAudible ? 'Every channel is silent. Turn off a Mute or a Solo to hear the file.' : ''}
          </p>
          {shown.some(s => s.muted || s.soloed || s.volume !== 1) && (
            <button
              type="button"
              className="self-start rounded px-1 type-small text-[hsl(var(--text2))] underline underline-offset-2 hover:text-foreground"
              onClick={() => onStrips(strips.map(() => ({ volume: 1, muted: false, soloed: false })))}
            >
              Reset channels
            </button>
          )}
        </div>
      )}
      {note && <p className="type-small text-foreground">{note}</p>}
      <p role="alert" className="type-small text-foreground empty:hidden">
        {notice}
      </p>
    </section>
  )
}
