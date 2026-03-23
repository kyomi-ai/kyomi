// SPDX-License-Identifier: AGPL-3.0-or-later

//! Input component — matches `apps/frontend/src/components/ui/input.jsx` exactly.

/// Input class string matching the React Input component exactly.
/// React: `flex h-9 w-full rounded-md border border-input bg-transparent px-3 py-1 text-base
///          text-foreground shadow-sm transition-colors file:border-0 file:bg-transparent
///          file:text-sm file:font-medium file:text-foreground placeholder:text-muted-foreground
///          focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring
///          disabled:cursor-not-allowed disabled:opacity-50 md:text-sm`
pub const INPUT_CLASS: &str = "flex h-9 w-full rounded-md border border-input bg-transparent px-3 py-1 text-base text-foreground shadow-sm transition-colors placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50 md:text-sm";
