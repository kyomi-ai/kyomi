# Button Standards

## Color System

The application uses a simple, consistent color palette:

### Primary Color: Amber/Orange
- **Hex**: `#d97706`
- **Tailwind**: `amber-600` / `amber-700` (hover)
- **Usage**: Primary actions, brand accents, active states
- **Examples**: Create buttons, Run buttons, Save buttons, primary CTAs, navigation highlights

### Secondary Color: Teal
- **Tailwind**: `teal-600` / `teal-700` (hover)
- **Usage**: AI/smart features, secondary intelligent actions
- **Examples**: SQL Copilot generate button, AI-powered features

### Everything Else: Shades of Gray
- **Backgrounds**: `gray-50` (page), `gray-100` (secondary buttons), `white` (cards)
- **Borders**: `gray-200` (soft), `gray-300` (emphasis when needed)
- **Text**: `gray-600` (secondary), `gray-800` (primary), `gray-900` (headings)
- **Usage**: All other UI elements, borders, backgrounds, text

### Semantic Colors (Standard UX Conventions)
- **Success**: `green-600` - Success states, confirmations, checkmarks
- **Error/Danger**: `red-600` - Errors, destructive actions, warnings
- **Warning**: `yellow-600` - Cautions, alerts (use sparingly)

**Design Philosophy**: Keep it simple. Amber for primary actions, teal for AI features, gray for everything else. This creates a clean, professional interface with clear visual hierarchy.

## Standard Button Sizes

All buttons in the application should follow these standard sizes for consistency:

### Standard Button (Most Common)
Use for primary and secondary actions throughout the app.

```jsx
// Primary action (amber/orange)
className="px-4 py-2 text-sm font-medium text-white bg-amber-600 hover:bg-amber-700 rounded-lg transition-colors"

// Secondary action (gray)
className="px-4 py-2 text-sm font-medium bg-gray-100 text-gray-700 hover:bg-gray-200 rounded-lg transition-colors"
```

**Specifications:**
- Padding: `px-4 py-2`
- Text: `text-sm font-medium`
- Border radius: `rounded-lg`
- Transition: `transition-colors`

### Small Button (Compact Actions)
Use for inline actions, toolbar buttons, or when space is limited.

```jsx
// Primary small button
className="px-3 py-1.5 text-xs font-medium bg-amber-600 text-white hover:bg-amber-700 rounded-md transition-colors"

// AI feature small button
className="px-3 py-1.5 text-xs font-medium bg-teal-600 text-white hover:bg-teal-700 rounded-md transition-colors"
```

**Specifications:**
- Padding: `px-3 py-1.5`
- Text: `text-xs font-medium`
- Border radius: `rounded-md`

### Icon Buttons
Use for icon-only buttons (no text).

```jsx
className="p-2 text-gray-400 hover:text-gray-600 hover:bg-gray-50 rounded transition-colors"
```

**Specifications:**
- Padding: `p-2` (equal padding all sides)
- No explicit text size (uses icon size)
- Border radius: `rounded`

## Button Colors by Type

### Primary Actions (Amber)
Use amber/orange for primary CTAs and main actions:
- Background: `bg-amber-600 hover:bg-amber-700`
- Text: `text-white`
- Examples: Create, Save, Run Query, Submit

### Secondary Actions (Gray)
Use gray for secondary/neutral actions:
- Background: `bg-gray-100 hover:bg-gray-200`
- Text: `text-gray-700`
- Examples: Cancel, Edit, View Details

### AI/Smart Features (Teal)
Use teal for AI-powered features:
- Background: `bg-teal-600 hover:bg-teal-700`
- Text: `text-white`
- Examples: Generate SQL, AI Assistant, Smart Suggestions

### Danger Actions (Red)
Use red for destructive actions:
- Background: `bg-red-600 hover:bg-red-700`
- Text: `text-white`
- Examples: Delete, Remove, Destroy

## Examples

### Dashboard Header Button
```jsx
<button
  onClick={() => navigate('/dashboard/new/edit')}
  className="flex items-center gap-2 px-4 py-2 text-sm font-medium text-white bg-amber-600 hover:bg-amber-700 rounded-lg transition-colors"
>
  <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
    <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 4v16m8-8H4" />
  </svg>
  Create Dashboard
</button>
```

### Modal Action Buttons
```jsx
<div className="flex gap-3">
  <button
    type="button"
    onClick={onCancel}
    className="flex-1 px-4 py-2 text-sm font-medium bg-gray-100 text-gray-700 hover:bg-gray-200 rounded-lg transition-colors"
  >
    Cancel
  </button>
  <button
    type="submit"
    className="flex-1 px-4 py-2 text-sm font-medium text-white bg-amber-600 hover:bg-amber-700 rounded-lg transition-colors"
  >
    Save
  </button>
</div>
```

### Card Action Buttons
```jsx
<div className="flex gap-2">
  <button
    className="flex-1 px-4 py-2 text-sm font-medium text-white bg-amber-600 hover:bg-amber-700 rounded-lg transition-colors"
  >
    View
  </button>
  <button
    className="flex-1 px-4 py-2 text-sm font-medium bg-gray-100 text-gray-700 hover:bg-gray-200 rounded-lg transition-colors"
  >
    Edit
  </button>
</div>
```

## Icons

When including icons with buttons:
- Standard button icons: `w-4 h-4`
- Small button icons: `w-3 h-3`
- Large emphasis icons: `w-5 h-5` (use sparingly)

## Disabled State

Always include disabled state styling:

```jsx
className="... disabled:opacity-50 disabled:cursor-not-allowed"
disabled={isLoading}
```

## Notes

- Always use `transition-colors` for smooth hover effects
- Never use inline styles for colors - use Tailwind classes (except for custom collection colors)
- Use `flex items-center gap-2` for buttons with icons and text
- Use `justify-center` for centered button content when appropriate
