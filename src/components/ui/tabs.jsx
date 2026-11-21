import * as React from 'react'
import * as TabsPrimitive from '@radix-ui/react-tabs'
import { cn } from '../../lib/utils'

const Tabs = TabsPrimitive.Root

const TabsList = React.forwardRef(({ className, ...props }, ref) => (
  <TabsPrimitive.List
    ref={ref}
    className={cn(
      'inline-flex items-center gap-0.5 rounded-lg bg-[hsl(var(--surface))] border border-border p-[3px]',
      className,
    )}
    {...props}
  />
))
TabsList.displayName = 'TabsList'

const TabsTrigger = React.forwardRef(({ className, ...props }, ref) => (
  <TabsPrimitive.Trigger
    ref={ref}
    className={cn(
      'inline-flex items-center justify-center gap-1.5 whitespace-nowrap rounded-md px-4 py-1.5',
      'border border-transparent type-label text-[hsl(var(--sub))] transition-[color,background-color,border-color,transform] duration-150 active:translate-y-px',
      'hover:text-[hsl(var(--text2))]',
      // Active = an ink frame on the quiet fill, in ink. Gold is kept for the
      // one primary action per view (depo-stack/BRAND.md §3), never a selection.
      'data-[state=active]:border-foreground data-[state=active]:bg-secondary data-[state=active]:text-foreground',
      className,
    )}
    {...props}
  />
))
TabsTrigger.displayName = 'TabsTrigger'

const TabsContent = React.forwardRef(({ className, ...props }, ref) => (
  <TabsPrimitive.Content ref={ref} className={cn('flex min-h-0 flex-1 flex-col', className)} {...props} />
))
TabsContent.displayName = 'TabsContent'

export { Tabs, TabsList, TabsTrigger, TabsContent }
