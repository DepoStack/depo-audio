import { Badge } from '../ui/badge'

export default function StatusChip({ status }) {
  const map = {
    waiting: ['default', 'Waiting'],
    queued: ['default', 'Queued'],
    converting: ['active', '● Processing'],
    done: ['done', '✓ Done'],
    error: ['error', '✗ Failed'],
  }
  const [variant, label] = map[status] || map.waiting
  return <Badge variant={variant}>{label}</Badge>
}
