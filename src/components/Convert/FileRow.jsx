import { useId, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { CH_COLORS } from '../../constants'
import { FileAudio, X, FolderOpen, ChevronDown, ChevronUp, Play, ArrowLeftRight, CircleSlash } from 'lucide-react'
import { Badge } from '../ui/badge'
import { cn } from '../../lib/utils'
import StatusChip from '../common/StatusChip'
import MiniPlayer from './MiniPlayer'
import ComparePlayer from '../common/ComparePlayer'
import {
  COMPARE_OUTPUT_UNAVAILABLE,
  COMPARE_UNAVAILABLE_REASON,
  compareTarget,
  isPlayerPlayable,
} from '../../lib/player'
import { audioTrackNotice, dmNotice } from '../../lib/queue'

export default function FileRow({ file, streams, job, onRemove, converting, onOpenInPlayer }) {
  const [expanded, setExpanded] = useState(false)
  const [folderError, setFolderError] = useState('')
  const [comparing, setComparing] = useState(false)
  const compareReasonId = useId()
  const status = job?.status || 'waiting'
  const isExp = file.fmt?.status === 'experimental'
  const isRej = file.fmt?.status === 'unsupported'
  const trackNotice = isRej ? '' : audioTrackNotice(streams)
  const recordedOverNotice = isRej ? '' : dmNotice(streams)

  return (
    <div
      aria-busy={status === 'converting'}
      className={cn(
        'bg-card border border-border rounded-lg overflow-hidden transition-colors',
        status === 'converting' && 'border-foreground',
        status === 'done' && 'border-success/40',
        status === 'error' && 'border-destructive/40',
        status === 'cancelled' && 'border-warning/40',
        isRej && 'opacity-60',
      )}
    >
      <div className="flex items-center gap-3 px-3 py-2.5">
        <div
          className={cn(
            'text-[hsl(var(--sub))] shrink-0',
            status === 'done' && 'text-success',
            status === 'error' && 'text-destructive',
            status === 'cancelled' && 'text-warning',
            status === 'converting' && 'text-foreground',
            isRej && 'text-destructive/60',
          )}
        >
          <FileAudio size={18} />
        </div>
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2">
            <span className="text-[13px] font-semibold text-foreground truncate" title={file.path}>
              {file.name}
            </span>
            {file.fmt && (
              <Badge variant={isRej ? 'error' : isExp ? 'warning' : 'tag'}>{file.fmt.name.split('·')[0].trim()}</Badge>
            )}
          </div>
          <span className="type-small text-[hsl(var(--sub))] truncate block">{file.path}</span>
          {trackNotice && <span className="type-small text-[hsl(var(--text2))] block mt-0.5">{trackNotice}</span>}
          {recordedOverNotice && (
            <span className="type-small text-[hsl(var(--text2))] block mt-0.5">{recordedOverNotice}</span>
          )}
        </div>
        <div className="flex items-center gap-2 shrink-0">
          <StatusChip status={status} />
          {!converting && (
            <button
              className="w-5 h-5 rounded flex items-center justify-center text-[hsl(var(--sub))] hover:text-destructive hover:bg-destructive/10 transition-colors"
              aria-label="Remove file"
              onClick={onRemove}
            >
              <X size={9} />
            </button>
          )}
        </div>
      </div>
      {status === 'converting' &&
        (() => {
          // Real percentage when the backend knows the source duration; capped
          // at 99% because trim can shorten the output — a bar sitting at 100%
          // while work continues reads as stuck. AI phases (no seconds yet)
          // stay indeterminate.
          const pct = job?.total > 0 && job?.seconds > 0 ? Math.min((job.seconds / job.total) * 100, 99) : null
          const phaseLabel =
            job?.phase === 'analyzing'
              ? 'Analyzing audio…'
              : job?.phase === 'processing'
                ? 'Removing noise…'
                : 'Encoding…'
          const progressText = `${phaseLabel}${
            pct != null ? ` ${Math.round(pct)}%` : job?.seconds > 0 ? ` ${Math.round(job.seconds)}s` : ''
          }`
          return (
            <div className="px-3 pb-2.5">
              {(job?.phase || pct != null || job?.seconds > 0) && (
                <span className="type-small text-[hsl(var(--sub))] block mb-1">{progressText}</span>
              )}
              <div
                role="progressbar"
                aria-label={`Conversion progress for ${file.name}`}
                aria-valuemin={0}
                aria-valuemax={100}
                aria-valuenow={pct == null ? undefined : Math.round(pct)}
                aria-valuetext={progressText}
                className="w-full h-1 bg-border overflow-hidden"
              >
                {pct != null ? (
                  <div
                    className="h-full bg-foreground transition-all motion-reduce:transition-none duration-300"
                    style={{ width: `${pct}%` }}
                  />
                ) : (
                  <div className="h-full bg-foreground animate-[loading_1.2s_ease-in-out_infinite] motion-reduce:animate-none" />
                )}
              </div>
            </div>
          )
        })()}
      {status === 'done' && job.outputs?.length > 0 && (
        <div className="px-3 pb-2.5 flex flex-col gap-0.5">
          {job.outputs.map((out, i) => (
            <MiniPlayer key={out.path} out={out} color={CH_COLORS[i % 4]} multi={job.outputs.length > 1} />
          ))}
          {(() => {
            const playable = job.outputs.map(out => out.path).filter(isPlayerPlayable)
            const compare = compareTarget(file.path, job.outputs, { trimmed: job.trimmed })
            const actionClass =
              'flex items-center gap-1 rounded px-1 py-0.5 type-small text-[hsl(var(--text2))] hover:text-foreground hover:bg-secondary/60 transition-colors focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring'
            return (
              <div className="flex flex-wrap items-center gap-2 mt-1">
                <button
                  type="button"
                  className={actionClass}
                  // Accessible names start with the visible label (WCAG 2.5.3)
                  aria-label={
                    job.outputs.length > 1
                      ? `Reveal in folder: the ${job.outputs.length} outputs of ${file.name}`
                      : `Reveal in folder: ${job.outputs[0].name}`
                  }
                  onClick={async () => {
                    setFolderError('')
                    try {
                      await invoke('show_in_folder', { path: job.outputs[0].path })
                    } catch (error) {
                      setFolderError(`Could not reveal the output folder: ${String(error)}`)
                    }
                  }}
                >
                  <FolderOpen size={11} aria-hidden="true" />
                  Reveal in folder
                </button>
                {onOpenInPlayer && playable.length > 0 && (
                  <button
                    type="button"
                    className={actionClass}
                    aria-label={
                      playable.length > 1
                        ? `Open in Player: the ${playable.length} outputs of ${file.name}`
                        : `Open in Player: ${job.outputs.find(out => out.path === playable[0])?.name}`
                    }
                    onClick={() => onOpenInPlayer(playable)}
                  >
                    <Play size={11} aria-hidden="true" />
                    Open in Player
                  </button>
                )}
                {/* Unavailable Compare stays focusable (aria-disabled) so
                    keyboard and screen-reader users reach its reason; it is
                    struck through with a slashed-circle icon, and its reason
                    sits after it in parentheses, so it does not read as
                    another action. */}
                <span className="flex flex-wrap items-center gap-1">
                  <button
                    type="button"
                    className={cn(
                      actionClass,
                      !compare.enabled &&
                        'cursor-not-allowed line-through hover:bg-transparent hover:text-[hsl(var(--text2))]',
                    )}
                    aria-label={`Compare with original: ${compare.output?.name ?? file.name}`}
                    aria-expanded={compare.enabled ? comparing : undefined}
                    aria-disabled={compare.enabled ? undefined : 'true'}
                    aria-describedby={compare.enabled ? undefined : compareReasonId}
                    onClick={() => {
                      if (compare.enabled) setComparing(open => !open)
                    }}
                  >
                    {compare.enabled ? (
                      <ArrowLeftRight size={11} aria-hidden="true" />
                    ) : (
                      <CircleSlash size={11} aria-hidden="true" />
                    )}
                    Compare with original
                  </button>
                  {!compare.enabled && (
                    <span id={compareReasonId} className="type-small text-[hsl(var(--sub))]">
                      (unavailable: {compare.reason})
                    </span>
                  )}
                </span>
              </div>
            )
          })()}
          {comparing &&
            (() => {
              const compare = compareTarget(file.path, job.outputs, { trimmed: job.trimmed })
              if (!compare.enabled) return null
              return (
                <ComparePlayer
                  originalPath={file.path}
                  processedPath={compare.output.path}
                  originalLabel="Original"
                  processedLabel="Output"
                  unavailableMessage={COMPARE_UNAVAILABLE_REASON}
                  processedUnavailableMessage={COMPARE_OUTPUT_UNAVAILABLE}
                  note={compare.note}
                  aligned={compare.aligned}
                />
              )
            })()}
          {folderError && (
            <p
              role="alert"
              className="mt-1 rounded border-l-[3px] border-l-destructive px-2 py-1.5 type-small text-destructive"
            >
              {folderError}
            </p>
          )}
        </div>
      )}
      {status === 'done' && job.warning && (
        <p role="alert" className="mx-3 mb-2.5 rounded bg-warning/10 px-2 py-1.5 type-small text-warning">
          {job.warning}
        </p>
      )}
      {status === 'cancelled' && (
        <p className="px-3 pb-2.5 type-small text-[hsl(var(--sub))]">
          {job.error || 'Conversion cancelled. No partial output was kept.'}
        </p>
      )}
      {status === 'error' && (
        <div className="px-3 pb-2.5">
          <button
            className="type-small text-[hsl(var(--sub))] hover:text-foreground transition-colors flex items-center gap-1"
            onClick={() => setExpanded(e => !e)}
          >
            {expanded ? (
              <>
                <ChevronUp size={10} /> hide
              </>
            ) : (
              <>
                <ChevronDown size={10} /> details
              </>
            )}
          </button>
          {expanded && (
            <pre className="mt-1 type-code text-destructive whitespace-pre-wrap break-all border-l-[3px] border-l-destructive rounded p-2">
              {job.error}
            </pre>
          )}
        </div>
      )}
    </div>
  )
}
