# Kyomi Design System

**Last Updated:** 2025-10-30
**Status:** Official Design System - All new features MUST follow these guidelines

## Purpose

This document is the **single source of truth** for all UI design decisions in Kyomi. When building new features or modifying existing ones, always reference this document first. DO NOT introduce new colors, spacing values, or component styles without updating this document.

---

## 🎨 Color System

### Semantic Color Tokens (Required)

All components MUST use these semantic tokens. These map to CSS variables defined in `apps/frontend/src/index.css`.

```css
/* Usage in components: bg-primary, text-primary-foreground, etc. */
--primary: #d97706 /* Amber - Main brand color for actions, links, primary buttons */
--primary-foreground: #ffffff /* White text on primary */

--secondary: #cbd5e1 /* Light slate - Secondary actions, less prominent buttons */
--secondary-foreground: #334155 /* Dark slate text on secondary */

--accent: #f1f5f9 /* Very light blue-gray - Hover states, focused elements */
--accent-foreground: #0f172a /* Dark text on accent */

--destructive: #ef4444 /* Red - Dangerous actions (delete, remove) */
--destructive-foreground: #ffffff /* White text on destructive */

--muted: #f1f5f9 /* Light gray - Disabled states, inactive elements */
--muted-foreground: #64748b /* Gray text for secondary text, labels, captions */

--background: #ffffff /* White - Page background color */
--foreground: #0f172a /* Near-black - Primary text color */

--card: #ffffff /* White - Card/panel backgrounds */
--card-foreground: #0f172a /* Dark text in cards */

--popover: #ffffff /* White - Dropdown, menu backgrounds */
--popover-foreground: #0f172a /* Dark text in dropdowns/menus */

--border: #e2e8f0 /* Light slate - Default border color */
--input: #e2e8f0 /* Light slate - Input field border color */
--ring: #d97706 /* Amber - Focus ring color */
```

**Key Color Decisions:**
- **Primary (Amber #d97706)**: Used for all primary actions (Run Query, Create Chart, New Chat). This is our brand color.
- **Secondary (Light Slate #cbd5e1)**: Used for secondary actions like AI Copilot Generate/Refine buttons. Lighter and more subtle than primary, doesn't compete for attention.
- **Text Contrast**: Primary uses white text, secondary uses dark slate text for proper accessibility.

### Status Colors (Semantic Tokens - Required)

**IMPORTANT:** Always use semantic status tokens, NEVER hardcoded Tailwind colors.

```css
/* Defined in apps/frontend/src/index.css */
--warning: #fff7ed          /* Light orange background */
--warning-foreground: #ea580c  /* Orange text */
--warning-border: #fed7aa   /* Orange border */

--error: #fff1f2            /* Light rose background */
--error-foreground: #e11d48 /* Rose text */
--error-border: #fecdd3     /* Rose border */

--success: #f0fdf4          /* Light green background */
--success-foreground: #15803d  /* Green text */
--success-border: #bbf7d0   /* Green border */

--info: #eff6ff             /* Light blue background */
--info-foreground: #2563eb  /* Blue text */
--info-border: #bfdbfe      /* Blue border */
```

**Usage:**
```jsx
// ✅ CORRECT - Use semantic tokens
<div className="bg-warning text-warning-foreground border border-warning-border">
  Warning message
</div>

// ❌ WRONG - Hardcoded Tailwind colors
<div className="bg-orange-50 text-orange-600 border-orange-200">
  Warning message
</div>
```

**Use the Status Components (see below)** instead of manually creating status displays.

### Chart Colors

For data visualizations, use the predefined palettes from `apps/frontend/src/config/chartPalettes.js`:

- **autumnForest** - Default palette, warm and professional
- **spectrumPro** - Vibrant, high contrast
- **horizonSuite** - Cool, modern palette

Each palette contains 12 colors optimized for data visualization and colorblind accessibility.

### ❌ NEVER Do This

```jsx
// ❌ WRONG - Hard-coded hex colors
<button className="bg-[#d97706] text-white">New Chat</button>

// ❌ WRONG - Direct Tailwind gray scale instead of semantic tokens
<div className="bg-gray-100 border-gray-300">

// ❌ WRONG - Random colors not in the design system
<span className="text-purple-500">
```

### ✅ ALWAYS Do This

```jsx
// ✅ CORRECT - Semantic tokens
<Button variant="default">New Chat</Button>

// ✅ CORRECT - Using semantic tokens for custom styling
<div className="bg-card border-border">

// ✅ CORRECT - Status colors for alerts/indicators
<div className="bg-amber-50 border-amber-200 text-amber-600">
```

---

## 📏 Spacing System

Use Tailwind's standard spacing scale exclusively. Never use arbitrary values unless absolutely necessary.

### Standard Spacing Scale

```
1  = 0.25rem (4px)
2  = 0.5rem  (8px)
3  = 0.75rem (12px)
4  = 1rem    (16px)
6  = 1.5rem  (24px)
8  = 2rem    (32px)
12 = 3rem    (48px)
16 = 4rem    (64px)
```

### Component Spacing Guidelines

| Context | Padding | Gap | Margin |
|---------|---------|-----|--------|
| **Cards** | `p-6` | - | - |
| **Modal Header** | `px-6 py-4` | - | - |
| **Modal Content** | `p-6` | - | - |
| **Modal Footer** | `px-6 py-4` | `gap-2` | - |
| **Status Bars** | `px-6 py-3.5` | `gap-4` | - |
| **Buttons** | `px-4 py-2` | `gap-2` | - |
| **Input Fields** | `px-3 py-1` | - | - |
| **Section Spacing** | - | `gap-4` or `gap-6` | `mb-4` or `mb-6` |
| **Inline Elements** | - | `gap-1.5` or `gap-2` | `ml-2` or `mr-2` |

### ❌ NEVER Do This

```jsx
// ❌ WRONG - Arbitrary spacing values
<div className="pl-[10px] pr-[15px]">

// ❌ WRONG - Inconsistent spacing
<button className="px-5 py-3"> // Should be px-4 py-2
```

### ✅ ALWAYS Do This

```jsx
// ✅ CORRECT - Standard spacing scale
<div className="px-6 py-4">

// ✅ CORRECT - Consistent button padding
<Button className="px-4 py-2">
```

---

## 🔤 Typography

### Font Stack

```css
/* Defined in apps/frontend/src/index.css */
--font-sans: system-ui, -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto,
             'Helvetica Neue', Arial, sans-serif;
--font-mono: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas,
             'Liberation Mono', 'Courier New', monospace;
```

### Font Sizes & Weights

| Element | Classes | Usage |
|---------|---------|-------|
| **Headings** | `text-xl font-semibold` | Page titles, modal titles |
| **Subheadings** | `text-lg font-semibold` | Section titles, card titles |
| **Body Text** | `text-base font-normal` | Paragraphs, descriptions |
| **Labels** | `text-sm font-medium` | Form labels, nav items |
| **Captions** | `text-sm text-muted-foreground` | Helper text, secondary info |
| **Small Text** | `text-xs` | Badges, timestamps, metadata |

### Text Colors

```
text-foreground          → Primary text
text-muted-foreground    → Secondary text, labels, disabled text
text-card-foreground     → Text inside cards
text-primary             → Links, emphasized text
text-destructive         → Error messages
```

### ❌ NEVER Do This

```jsx
// ❌ WRONG - Arbitrary font sizes
<h2 className="text-[18px]">Title</h2>

// ❌ WRONG - Inconsistent heading styles
<h2 className="text-2xl font-bold"> // Should be text-xl font-semibold
```

### ✅ ALWAYS Do This

```jsx
// ✅ CORRECT - Standard typography classes
<h2 className="text-xl font-semibold">Title</h2>
<p className="text-sm text-muted-foreground">Description</p>
```

---

## 🧩 Component Library

All UI components are located in `apps/frontend/src/components/ui/` and follow the shadcn/ui pattern.

### Button Component

**Location:** `apps/frontend/src/components/ui/button.jsx`

```jsx
import { Button } from '@/components/ui/button';

// Variants
<Button variant="default">Primary Action</Button>
<Button variant="secondary">Secondary Action</Button>
<Button variant="outline">Outlined</Button>
<Button variant="ghost">Subtle</Button>
<Button variant="destructive">Delete</Button>
<Button variant="link">Link Style</Button>

// Sizes
<Button size="default">Default</Button>
<Button size="sm">Small</Button>
<Button size="lg">Large</Button>
<Button size="icon"><IconComponent /></Button>
```

**DO NOT create custom button styles** - Use the Button component with variants.

### Card Component

**Location:** `apps/frontend/src/components/ui/card.jsx`

```jsx
import { Card, CardHeader, CardTitle, CardDescription, CardContent, CardFooter } from '@/components/ui/card';

<Card>
  <CardHeader>
    <CardTitle>Card Title</CardTitle>
    <CardDescription>Card description or subtitle</CardDescription>
  </CardHeader>
  <CardContent>
    {/* Card content */}
  </CardContent>
  <CardFooter>
    {/* Actions */}
  </CardFooter>
</Card>
```

### Input Component

**Location:** `apps/frontend/src/components/ui/input.jsx`

```jsx
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';

<div>
  <Label htmlFor="email">Email</Label>
  <Input id="email" type="email" placeholder="you@example.com" />
</div>
```

### Select Component

**Location:** `apps/frontend/src/components/ui/select.jsx`

```jsx
import { Select, SelectTrigger, SelectValue, SelectContent, SelectItem } from '@/components/ui/select';

<Select onValueChange={handleChange}>
  <SelectTrigger>
    <SelectValue placeholder="Select option" />
  </SelectTrigger>
  <SelectContent>
    <SelectItem value="option1">Option 1</SelectItem>
    <SelectItem value="option2">Option 2</SelectItem>
  </SelectContent>
</Select>
```

### Badge Component

**Location:** `apps/frontend/src/components/ui/badge.jsx`

```jsx
import { Badge } from '@/components/ui/badge';

<Badge variant="default">Default</Badge>
<Badge variant="secondary">Secondary</Badge>
<Badge variant="destructive">Error</Badge>
<Badge variant="outline">Outline</Badge>
```

### Alert Component (Status Messages)

**Location:** `apps/frontend/src/components/ui/alert.jsx`

For inline status messages, error displays, and notifications:

```jsx
import { Alert, AlertTitle, AlertDescription } from '@/components/ui/alert';

// Variants
<Alert variant="default">Default notification</Alert>
<Alert variant="warning">Warning message</Alert>
<Alert variant="error">Error message</Alert>
<Alert variant="success">Success message</Alert>
<Alert variant="info">Info message</Alert>

// With title and description
<Alert variant="error">
  <AlertTitle>Error Title</AlertTitle>
  <AlertDescription>
    Detailed error message goes here with helpful context.
  </AlertDescription>
</Alert>
```

**When to use:**
- Error messages from API calls
- Form validation errors
- User notifications within content areas
- Inline warnings or success confirmations

**DO NOT** create custom alert boxes - use the Alert component with appropriate variant.

### StatusBadge Component (Inline Indicators)

**Location:** `apps/frontend/src/components/ui/status-badge.jsx`

For small, inline status indicators (like pills or tags):

```jsx
import { StatusBadge } from '@/components/ui/status-badge';

// Variants
<StatusBadge variant="default">Pending</StatusBadge>
<StatusBadge variant="warning">Needs Attention</StatusBadge>
<StatusBadge variant="error">Failed</StatusBadge>
<StatusBadge variant="success">Complete</StatusBadge>
<StatusBadge variant="info">Processing</StatusBadge>

// Example: Token status with custom content
<StatusBadge variant="success" className="gap-2">
  <span>✓</span>
  <span>Authenticated</span>
  <span className="text-xs opacity-75">(user@example.com)</span>
</StatusBadge>
```

**When to use:**
- Token/connection status indicators
- Workflow state badges
- Quick-glance status labels
- List item status tags

### StatusBar Component (Prominent Notifications)

**Location:** `apps/frontend/src/components/ui/status-bar.jsx`

For full-width, prominent notification bars (typically at bottom/top of screen):

```jsx
import { StatusBar } from '@/components/ui/status-bar';

// Variants
<StatusBar variant="warning">
  <div className="flex items-center gap-2">
    <span className="font-medium">Connection Warning</span>
    <span>Your session will expire in 5 minutes.</span>
  </div>
  <button className="underline hover:no-underline">Extend Session</button>
</StatusBar>

<StatusBar variant="error">
  <span>OAuth token expired. Please reconnect.</span>
  <button onClick={handleReconnect}>Reconnect</button>
</StatusBar>

<StatusBar variant="success">
  Dashboard saved successfully!
</StatusBar>

<StatusBar variant="info">
  Catalog refresh in progress...
</StatusBar>
```

**When to use:**
- OAuth/connection status at bottom of screen
- Global notifications that need high visibility
- Session/token expiration warnings
- System-wide alerts or announcements

**Layout:** Always full-width with `px-6 py-3.5` padding, `border-t`, and flex layout for content + actions.

### EmptyState Component (No Data Displays)

**Location:**
- React: `apps/frontend/src/components/ui/empty-state.jsx`
- Leptos: `crates/kyomi-ui/src/components/empty_state.rs`

For displaying empty states when no data is available. ALWAYS use the shared
EmptyState component — never build inline empty states with ad-hoc markup.

**Leptos usage:**
```rust
use crate::components::{EmptyState, EmptyStateVariant};

// Basic
view! {
    <EmptyState
        title="No results found"
        description="Try adjusting your search criteria"
    />
}

// With variant, icon, and action
view! {
    <EmptyState
        variant=EmptyStateVariant::Info
        icon=|| view! { <Icon icon=icondata_lu::LuDatabase width="48" height="48"/> }
        title="Get started"
        description="Connect a data source to begin"
        action=|| view! {
            <Button on:click=|_| {}>"Connect Datasource"</Button>
        }
    />
}
```

**Variants:** `Default`, `Warning`, `Error`, `Success`, `Info`

**When to use:**
- Empty chart grids
- Empty table/list displays
- No search results
- Onboarding states for new users

### Other Available Components

All located in `apps/frontend/src/components/ui/`:

- `avatar.jsx` - User avatars with fallback
- `breadcrumb.jsx` - Navigation breadcrumbs
- `separator.jsx` - Horizontal/vertical dividers
- `skeleton.jsx` - Loading placeholders
- `sheet.jsx` - Slide-out panels/sidebars
- `table.jsx` - Data tables
- `tooltip.jsx` - Hover tooltips

---

## 📦 Layout Patterns

### Modal Component (Center Overlays)

**Location:** `apps/frontend/src/components/Modal.jsx`

For forms, confirmations, multi-step workflows, and content requiring immediate attention.

```jsx
import Modal from '../components/Modal';
import { Button } from '../components/ui/button';

// ✅ CORRECT - Use the Modal component
const MyFeature = () => {
  const [isOpen, setIsOpen] = useState(false);
  const [formData, setFormData] = useState('');

  const handleSave = () => {
    // Save logic
    setIsOpen(false);
  };

  return (
    <>
      <Button onClick={() => setIsOpen(true)}>Open Modal</Button>

      <Modal
        show={isOpen}
        onClose={() => setIsOpen(false)}
        title="Modal Title"
        size="lg"
        footer={
          <>
            <Button variant="outline" onClick={() => setIsOpen(false)}>
              Cancel
            </Button>
            <Button variant="default" onClick={handleSave}>
              Save
            </Button>
          </>
        }
      >
        {/* Modal content */}
        <input
          type="text"
          value={formData}
          onChange={(e) => setFormData(e.target.value)}
          className="w-full px-3 py-2 border border-gray-300 rounded-lg focus:ring-2 focus:ring-primary"
        />
      </Modal>
    </>
  );
};
```

**Modal Sizes:**
- `sm` - Small (384px) - Confirmations, simple forms
- `md` - Medium (448px) - Single-field forms
- `lg` - Large (896px) - Default, multi-field forms
- `xl` - Extra large (1152px) - Complex forms, tables
- `full` - Full width (95vw) - Maximum space needed

**Features:**
- Backdrop click closes modal
- Portal rendering (escapes DOM hierarchy)
- Standardized close button (Button variant="ghost" size="icon")
- Header, content, and footer sections
- Responsive sizing

### Sheet Component (Slide-out Panels)

**Location:** `apps/frontend/src/components/ui/sheet.jsx`

For settings, filters, detail views, and contextual actions.

```jsx
import { Sheet, SheetTrigger, SheetContent, SheetHeader, SheetTitle, SheetFooter } from '@/components/ui/sheet';
import { Button } from '@/components/ui/button';

// ✅ CORRECT - Use Sheet for side panels
<Sheet>
  <SheetTrigger asChild>
    <Button variant="outline">Open Settings</Button>
  </SheetTrigger>
  <SheetContent side="right">
    <SheetHeader>
      <SheetTitle>Settings</SheetTitle>
    </SheetHeader>
    {/* Sheet content */}
    <SheetFooter>
      <Button>Save Changes</Button>
    </SheetFooter>
  </SheetContent>
</Sheet>
```

**Sheet Sides:**
- `right` - Default, settings and filters
- `left` - Navigation panels
- `top` - Notifications
- `bottom` - Mobile actions

**When to Use:**
- **Modal** → Center overlay for focused tasks (forms, confirmations)
- **Sheet** → Slide-out for contextual tools (settings, filters, details)

---

## 🎯 Modals, Dialogs & Notifications - Standards

**Status:** ✅ Approved
**Goal:** Unified, predictable UX for all overlays and notifications

### Standards

#### **1. Modal Backdrop Standard**

**Standard: 50% black backdrop, no blur**

```css
/* Standard defined in index.css */
.modal-overlay {
  @apply fixed inset-0 flex items-center justify-center z-50 font-sans bg-black/50;
}
```

**Why 50%:**
- Better visual focus on modal content
- Industry standard (Gmail, Slack, GitHub use 40-60%)
- Blur is unnecessary and hurts performance

**Implementation:** ✅ Complete - All modals now use 50% backdrop

---

#### **2. Toast Notifications**

**Purpose:** Non-blocking notifications that replace `alert()` browser dialogs

**Library:** Sonner (https://sonner.emilkowal.ski/)
**Location:** `apps/frontend/src/lib/toast.js` (wrapper utility)

**Usage:**
```jsx
import { toast } from '@/lib/toast';

// Success
toast.success('Dashboard saved successfully!');

// Error
toast.error('Failed to connect to BigQuery');

// Warning
toast.warning('Your session will expire in 5 minutes');

// Info
toast.info('New features available');

// With custom duration
toast.success('Copied!', { duration: 2000 });

// Loading state
const toastId = toast.loading('Saving dashboard...');
// Later: toast.success('Saved!', { id: toastId });

// Promise-based (automatically shows loading/success/error)
toast.promise(
  saveData(),
  {
    loading: 'Saving...',
    success: 'Dashboard saved!',
    error: 'Failed to save'
  }
);
```

**Features:**
- Non-blocking (appears in corner, auto-dismisses)
- Stackable (multiple toasts at once)
- Matches design system colors with `richColors` option
- Accessible (screen reader announcements)
- Dismissible (click X or auto-dismiss)
- Support for loading states and promise-based toasts

**Configuration:**
```jsx
// Already configured in App.jsx
<Toaster position="top-right" expand={false} richColors closeButton />
```

**Position:** Top-right corner (standard UX pattern)
**Duration:**
- Success/Warning/Info: 4 seconds
- Error: 5 seconds (longer for users to read)
- Loading: Until manually dismissed or replaced

**Status:** ✅ Complete - Replaced all 23 instances of `alert()` across 19 files

---

#### **3. ConfirmDialog**

**Purpose:** Styled confirmation dialogs that replace `confirm()` browser dialogs

**Location:** `apps/frontend/src/components/ConfirmDialog.jsx`
**Hook:** `apps/frontend/src/hooks/useConfirm.js`

**Usage:**
```jsx
import ConfirmDialog from '@/components/ConfirmDialog';
import useConfirm from '@/hooks/useConfirm';

function MyComponent() {
  const { isOpen, dialogProps, confirm } = useConfirm();

  const handleDelete = async () => {
    const confirmed = await confirm({
      title: 'Delete Chat?',
      message: 'This action cannot be undone. All messages will be permanently deleted.',
      confirmText: 'Delete',
      variant: 'destructive'
    });

    if (confirmed) {
      // Proceed with deletion
    }
  };

  return (
    <>
      <Button onClick={handleDelete}>Delete</Button>
      <ConfirmDialog isOpen={isOpen} {...dialogProps} />
    </>
  );
}
```

**Features:**
- Non-blocking (unlike browser `confirm()`)
- Matches design system
- Customizable (icons, descriptions, button variants)
- Keyboard navigation (Enter to confirm, Esc to cancel)
- Accessible (ARIA labels)
- Promise-based API for clean async/await code
- Smart focus: Cancel button for destructive actions, Confirm button for default

**Variants:**
- `destructive` - Red button for dangerous actions (delete, cancel subscription)
- `default` - Primary button for normal confirmations

**Status:** ✅ Complete - Replaced all 17 instances of `confirm()` across 13 files

---

### Decision Tree: When to Use What

```
┌─ Need user input or confirmation?
│
├─ YES → Use Modal or ConfirmDialog
│   │
│   ├─ Complex multi-step workflow? ────────────→ Modal
│   ├─ Form with multiple fields? ──────────────→ Modal
│   ├─ Simple yes/no decision? ─────────────────→ ConfirmDialog
│   └─ Destructive action (delete, cancel)? ────→ ConfirmDialog (destructive)
│
└─ NO → Use Alert or Toast
    │
    ├─ Critical error blocking workflow? ───────→ Alert (inline, persistent)
    ├─ Contextual warning/help? ────────────────→ Alert (inline, dismissible)
    ├─ Success/info notification? ──────────────→ Toast (auto-dismiss)
    └─ Background task completion? ─────────────→ Toast (auto-dismiss)
```

---

### Component Usage Guide

#### **Modal** (`<Modal>`) - Focused Tasks
**Use for:**
- ✅ Creating/editing dashboards
- ✅ Multi-step wizards
- ✅ Forms with multiple fields
- ✅ Chart builder
- ✅ SQL editor popups

**Don't use for:**
- ❌ Settings (use Sheet)
- ❌ Simple confirmations (use ConfirmDialog)
- ❌ Success notifications (use Toast)

**Example:**
```jsx
<Modal show={isOpen} onClose={handleClose} title="Create Dashboard" size="lg">
  <form onSubmit={handleSubmit}>
    <Input label="Dashboard Name" />
    <Input label="Description" />
    <Button type="submit">Create</Button>
  </form>
</Modal>
```

---

#### **Sheet** (`<Sheet>`) - Contextual Panels
**Use for:**
- ✅ Settings panels
- ✅ Filters and search
- ✅ Detail views
- ✅ Navigation drawers
- ✅ Contextual help

**Don't use for:**
- ❌ Forms requiring focus (use Modal)
- ❌ Critical confirmations (use ConfirmDialog)

**Example:**
```jsx
<Sheet>
  <SheetTrigger asChild>
    <Button variant="outline">Filters</Button>
  </SheetTrigger>
  <SheetContent side="right">
    <SheetHeader>
      <SheetTitle>Filter Results</SheetTitle>
    </SheetHeader>
    <FilterForm />
  </SheetContent>
</Sheet>
```

---

#### **ConfirmDialog** (`useConfirm()`) - Confirmations
**Use for:**
- ✅ Delete confirmations
- ✅ Destructive actions
- ✅ Yes/no decisions
- ✅ "Are you sure?" prompts
- ✅ Canceling subscriptions
- ✅ Logging out all devices

**Don't use for:**
- ❌ Success notifications (use Toast)
- ❌ Forms (use Modal)
- ❌ Informational messages (use Alert or Toast)

**Example:**
```jsx
const handleDelete = async () => {
  const confirmed = await confirm({
    title: 'Delete Dashboard?',
    message: 'This will permanently delete "Q4 Revenue Dashboard" and all its charts. This action cannot be undone.',
    confirmText: 'Delete Dashboard',
    confirmVariant: 'destructive'
  });

  if (confirmed) {
    await deleteDashboard(id);
    toast.success('Dashboard deleted');
  }
};
```

---

#### **Alert** (inline) - Persistent Messages
**Use for:**
- ✅ Form validation errors
- ✅ Persistent warnings
- ✅ Contextual help/info
- ✅ Error messages in specific sections
- ✅ Important notices that need attention

**Don't use for:**
- ❌ Success confirmations (use Toast)
- ❌ Temporary notifications (use Toast)
- ❌ Confirmations (use ConfirmDialog)

**Example:**
```jsx
<Alert variant="error">
  <AlertTitle>Connection Failed</AlertTitle>
  <AlertDescription>
    Unable to connect to BigQuery. Please check your OAuth token and try again.
  </AlertDescription>
</Alert>
```

---

#### **Toast** - Brief Notifications
**Use for:**
- ✅ Success confirmations ("Saved!")
- ✅ Background task completion
- ✅ Brief error notifications
- ✅ Copy-to-clipboard feedback
- ✅ Auto-save indicators

**Don't use for:**
- ❌ Critical errors (use Alert)
- ❌ Confirmations (use ConfirmDialog)
- ❌ Forms (use Modal)
- ❌ Persistent warnings (use Alert)

**Example:**
```jsx
// Quick success
toast.success('Dashboard saved!');

// Error with action
toast.error('Failed to save', {
  action: {
    label: 'Retry',
    onClick: () => handleSave()
  }
});

// Loading state
const toastId = toast.loading('Saving dashboard...');
await saveDashboard();
toast.success('Saved!', { id: toastId }); // Replace loading toast
```

---

### Anti-Patterns to Avoid

#### ❌ Don't Use Browser Native Dialogs
```javascript
// ❌ BAD - Blocks browser, ugly, inconsistent
alert('Successfully saved!');
confirm('Are you sure?');
prompt('Enter name:');

// ✅ GOOD - Use React components
toast.success('Successfully saved!');
await confirm({ title: 'Are you sure?' });
<Modal>...</Modal> // For text input
```

#### ❌ Don't Create Custom Modal Implementations
```jsx
// ❌ BAD - Custom backdrop and structure
<div className="fixed inset-0 bg-black bg-opacity-50">
  <div className="bg-white rounded-lg p-6">
    Custom modal content
  </div>
</div>

// ✅ GOOD - Use Modal component
<Modal show={isOpen} onClose={handleClose}>
  Modal content
</Modal>
```

#### ❌ Don't Mix Backdrop Styles
```jsx
// ❌ BAD - Inconsistent opacity
className="bg-black/20"  // Too light
className="bg-black/50"  // Good
className="bg-black/80"  // Too dark
className="bg-black/50 backdrop-blur-sm"  // Unnecessary blur

// ✅ GOOD - Use standard
className="modal-overlay"  // Consistent 50%
```

#### ❌ Don't Block Users Unnecessarily
```javascript
// ❌ BAD - Blocks for non-critical info
alert('Your dashboard has been saved!');

// ✅ GOOD - Non-blocking notification
toast.success('Dashboard saved!');
```

---

### Migration Notes

**Phase 1: Standardize Backdrop** (2-3 hours)
- [ ] Update `.modal-overlay` in `index.css` to `bg-black/50`
- [ ] Refactor ChartBuilderModal to use `<Modal>`
- [ ] Refactor SQLEditor to use `<Modal>`
- [ ] Remove custom backdrop from BillingPanel

**Phase 2: Add ConfirmDialog** (4-6 hours)
- [ ] Create `components/ui/confirm-dialog.jsx`
- [ ] Create `hooks/useConfirm.js`
- [ ] Add ConfirmProvider to app root
- [ ] Replace 17 `confirm()` calls with `useConfirm()`
- [ ] Test all destructive actions

**Phase 3: Add Toast Notifications** (2-3 hours)
- [ ] Install sonner or react-hot-toast
- [ ] Create toast wrapper matching design system
- [ ] Add toast provider to app root
- [ ] Replace 11 `alert()` calls with `toast()`
- [ ] Test all notification scenarios

**Phase 4: Documentation** (1-2 hours)
- [ ] Update this design system guide
- [ ] Create component examples page
- [ ] Add to component library
- [ ] Team training/announcement

**Total Estimated Effort:** 10-14 hours

---

### Card Grid Layout

```jsx
// ✅ CORRECT - Responsive card grid
<div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
  <Card>...</Card>
  <Card>...</Card>
  <Card>...</Card>
</div>
```

### Page Container Layout

```jsx
// ✅ CORRECT - Standard page layout
<div className="flex flex-col h-screen">
  {/* Header/Nav */}
  <header className="border-b border-border px-6 py-4">
    <h1 className="text-xl font-semibold">Page Title</h1>
  </header>

  {/* Main Content */}
  <main className="flex-1 overflow-auto p-6">
    <div className="max-w-7xl mx-auto space-y-6">
      {/* Page content */}
    </div>
  </main>
</div>
```

---

## 🎯 Borders & Shadows

### Border Styles

```jsx
// Default border
className="border border-border"

// Specific side borders
className="border-b border-border"  // Bottom only
className="border-t border-border"  // Top only

// Status borders (when appropriate)
className="border border-amber-200"  // Warning
className="border border-red-200"    // Error
className="border border-green-200"  // Success
```

### Border Radius

Strict 3-tier system. CSS tokens defined in `crates/kyomi-ui/style/main.css`.

```
Token          Class        Size   Usage
--radius-sm    rounded-md   6px    Inputs, buttons, chips, select items
--radius-md    rounded-lg   8px    Cards, dropdowns, panels, alerts
--radius-lg    rounded-xl   12px   Modals, dialogs, large containers
(none)         rounded-full 100%   Avatars, badges, switch track
```

**Rules:**
- Do NOT use bare `rounded` (without size suffix) — always specify `rounded-md` or `rounded-lg`
- Do NOT use inline `border-radius` styles — use Tailwind classes
- `rounded-2xl` is allowed ONLY for chat message bubbles (conversational UI convention)
- Directional variants (`rounded-t-lg`, `rounded-b-md`) follow the same tier they reference

### Shadow Levels

```
shadow-sm → Buttons, inputs
shadow    → Cards
shadow-lg → Sheets, sidebars
shadow-xl → Modals, overlays
```

---

## ♿ Accessibility & States

### Focus States

All interactive elements MUST have visible focus indicators:

```jsx
className="focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
```

### Disabled States

```jsx
className="disabled:opacity-50 disabled:cursor-not-allowed disabled:pointer-events-none"
```

### Hover States

```jsx
// Buttons
className="hover:bg-primary/90"

// Links
className="hover:text-foreground hover:underline"

// Cards (if clickable)
className="hover:bg-accent/50"
```

---

## 🚫 Common Anti-Patterns

### ❌ Hard-coded Colors

```jsx
// ❌ WRONG
<button className="bg-[#3b82f6] text-white">
<div className="bg-gray-100">
<span className="text-[#d97706]">
```

### ❌ Inconsistent Button Styles

```jsx
// ❌ WRONG - Custom button styling
<button className="bg-blue-500 hover:bg-blue-600 px-6 py-3 rounded-lg">
  Click me
</button>

// ✅ CORRECT - Use Button component
<Button variant="default" size="default">Click me</Button>
```

### ❌ Arbitrary Spacing

```jsx
// ❌ WRONG
<div className="px-[23px] py-[17px]">

// ✅ CORRECT
<div className="px-6 py-4">
```

### ❌ Multiple Modal Implementations

```jsx
// ❌ WRONG - Creating custom modal structure
const CustomModal = () => (
  <div className="fixed inset-0 bg-black bg-opacity-50">
    <div className="bg-white rounded-lg p-6">
      <h2>Custom Modal</h2>
      <button style={{backgroundColor: '#d97706'}}>Save</button>
    </div>
  </div>
);

// ✅ CORRECT - Use Modal component
import Modal from '../components/Modal';
import { Button } from '../components/ui/button';

const MyComponent = () => (
  <Modal
    show={isOpen}
    onClose={() => setIsOpen(false)}
    title="Standard Modal"
    size="md"
    footer={
      <Button variant="default">Save</Button>
    }
  >
    {/* Content */}
  </Modal>
);
```

**Why this matters:**
- Consistent UX across all modals
- Standardized close behavior (backdrop click, ESC key)
- Design system compliance (colors, spacing, buttons)
- No inline styles or hardcoded colors

---

## 📐 Responsive Design

### Breakpoints

```
sm: 640px   → Mobile landscape
md: 768px   → Tablet
lg: 1024px  → Desktop
xl: 1280px  → Large desktop
```

### Mobile-First Approach

```jsx
// ✅ CORRECT - Mobile first, enhance for larger screens
<div className="flex flex-col md:flex-row gap-4">
  <div className="w-full md:w-1/2">...</div>
  <div className="w-full md:w-1/2">...</div>
</div>
```

---

## 🔄 Animation & Transitions

### Design Tokens (defined in `crates/kyomi-ui/style/main.css`)

```css
/* Durations */
--duration-fast: 100ms;    /* Color changes, hover states */
--duration-normal: 200ms;  /* Position/size changes, mount animations */
--duration-slow: 300ms;    /* Major layout shifts, panel slides */

/* Easing curves */
--ease-out: cubic-bezier(0.16, 1, 0.3, 1);    /* Entering elements */
--ease-in: cubic-bezier(0.7, 0, 0.84, 0);      /* Exiting elements */
--ease-in-out: cubic-bezier(0.45, 0, 0.55, 1);  /* Moving elements */
```

These tokens generate Tailwind utility classes automatically:
`duration-fast`, `duration-normal`, `duration-slow`, `ease-out`, `ease-in`, `ease-in-out`.

### Standard Transitions

```rust
// Color transitions (buttons, links, hover states) — MANDATORY on all hover states
"transition-colors"  // uses Tailwind default 150ms, which is fine

// Panel/sidebar position/size changes
"transition-all duration-slow ease-in-out"

// Opacity fades
"transition-opacity duration-normal"
```

**Rule:** Every element with `hover:bg-*` or `hover:text-*` MUST have `transition-colors`.

### Entrance Animations

Utility classes for mount-time animations (defined in `main.css`):

```
Class                   Animation                     Use for
animate-fade-in         Fade in (200ms, ease-out)     Backdrop overlays, subtle reveals
animate-fade-in-fast    Fade in (100ms, ease-out)     Modal/dialog backdrops
animate-zoom-fade-in    Fade + scale 95→100%          Modals, dialogs, confirm dialogs
animate-slide-fade-in   Fade + slide from top         Dropdowns, selects, popovers
animate-slide-in-right  Fade + slide from right       Toast notifications
```

**Rule:** Every mount-animated element MUST use a named animation class — never inline durations.

### Reduced Motion

All animations and transitions are automatically disabled when the user has
`prefers-reduced-motion: reduce` enabled. This is handled globally in `main.css`.

### Loading States

Use the Skeleton component for loading placeholders:

```rust
use crate::components::Skeleton;

view! { <Skeleton class="h-10 w-full" /> }

<Skeleton className="h-10 w-full" />
```

---

## 📋 Checklist for New Features

Before implementing ANY new UI feature, verify:

- [ ] Using semantic color tokens (`bg-primary`, `text-muted-foreground`, `bg-warning`, etc.)
- [ ] Using semantic status tokens (`bg-error`, `text-success-foreground`, `border-warning-border`)
- [ ] Using existing Button component with appropriate variant
- [ ] Using Alert component for error/warning/success messages (NOT custom boxes)
- [ ] Using StatusBadge for inline status indicators
- [ ] Using StatusBar for prominent full-width notifications
- [ ] Using EmptyState for empty data displays
- [ ] Using standard spacing scale (no arbitrary values like `px-[23px]`)
- [ ] Using existing Card component for containers
- [ ] Following typography guidelines (`text-xl font-semibold`, etc.)
- [ ] Using Modal component for center overlays, Sheet component for slide-outs
- [ ] All interactive elements have focus states
- [ ] All disabled states use `disabled:opacity-50`
- [ ] Following mobile-first responsive approach
- [ ] Using existing UI components from `components/ui/` directory

---

## 🛠️ Implementation Guide for Claude Code

When Claude Code asks you to build a new feature:

1. **Start with:** "Before implementing, review `docs/DESIGN_SYSTEM.md` and use ONLY those design tokens."

2. **Be specific:** Instead of "make it look better", say "use the Button component with variant='default' and the standard text-sm font-medium for labels"

3. **Reference existing components:** "Use the Card component structure from design-system.md for this container"

4. **Enforce consistency:** "Do not introduce new colors, spacing values, or styles. Use only what's defined in DESIGN_SYSTEM.md"

---

## 📝 Updating This Document

This design system is a living document. When making changes:

1. **Propose changes first** - Discuss with team before adding new patterns
2. **Update consistently** - If adding a new color, add it to the CSS variables AND this document
3. **Document rationale** - Explain WHY a new pattern is needed
4. **Version control** - Commit design system changes alongside code changes
5. **Communicate changes** - Notify the team when design tokens change

---

## 🔗 Related Files

- **CSS Variables:** `apps/frontend/src/index.css`
- **Tailwind Config:** `apps/frontend/tailwind.config.js`
- **UI Components:** `apps/frontend/src/components/ui/`
- **Chart Palettes:** `apps/frontend/src/config/chartPalettes.js`
- **Utilities:** `apps/frontend/src/lib/utils.js` (contains `cn()` helper)

---

**Remember:** Consistency is MORE IMPORTANT than perfection. It's better to have a consistent "good enough" design system than a perfect but inconsistently applied one.
