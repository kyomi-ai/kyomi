// SPDX-License-Identifier: AGPL-3.0-or-later
import * as React from "react"
import { cva } from "class-variance-authority"
import { cn } from "@/lib/utils"

const statusBadgeVariants = cva(
  "inline-flex items-center rounded-full px-2.5 py-0.5 text-xs font-semibold transition-colors",
  {
    variants: {
      variant: {
        default: "bg-muted text-muted-foreground",
        warning: "bg-warning text-warning-foreground border border-warning-border",
        error: "bg-error text-error-foreground border border-error-border",
        success: "bg-success text-success-foreground border border-success-border",
        info: "bg-info text-info-foreground border border-info-border",
      },
    },
    defaultVariants: {
      variant: "default",
    },
  }
)

function StatusBadge({ className, variant, children, ...props }) {
  return (
    <span
      className={cn(statusBadgeVariants({ variant }), className)}
      {...props}
    >
      {children}
    </span>
  )
}

export { StatusBadge, statusBadgeVariants }
