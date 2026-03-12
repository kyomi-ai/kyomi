// SPDX-License-Identifier: AGPL-3.0-or-later
import * as React from "react"
import { cva } from "class-variance-authority"
import { cn } from "@/lib/utils"

const emptyStateVariants = cva(
  "rounded-lg border p-8 text-center",
  {
    variants: {
      variant: {
        default: "bg-background border-border",
        warning: "bg-warning border-warning-border",
        error: "bg-error border-error-border",
        success: "bg-success border-success-border",
        info: "bg-info border-info-border",
      },
    },
    defaultVariants: {
      variant: "default",
    },
  }
)

function EmptyState({ className, variant, icon, title, description, action, children, ...props }) {
  const textColorMap = {
    default: "text-muted-foreground",
    warning: "text-warning-foreground",
    error: "text-error-foreground",
    success: "text-success-foreground",
    info: "text-info-foreground",
  }

  return (
    <div
      className={cn(emptyStateVariants({ variant }), className)}
      {...props}
    >
      {icon && (
        <div className={cn("mx-auto mb-4 w-12 h-12 flex items-center justify-center", textColorMap[variant])}>
          {icon}
        </div>
      )}
      {title && (
        <h3 className={cn("text-base font-semibold mb-2", variant === "default" ? "text-foreground" : textColorMap[variant])}>
          {title}
        </h3>
      )}
      {description && (
        <p className={cn("text-sm mb-4", textColorMap[variant])}>
          {description}
        </p>
      )}
      {action && (
        <div className="mt-4">
          {action}
        </div>
      )}
      {children}
    </div>
  )
}

export { EmptyState, emptyStateVariants }
