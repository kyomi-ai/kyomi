// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState } from 'react';
import { Button } from '@/components/ui/button';
import { Card, CardHeader, CardTitle, CardDescription, CardContent } from '@/components/ui/card';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Badge } from '@/components/ui/badge';
import { StatusBadge } from '@/components/ui/status-badge';
import { StatusBar, StatusBarText } from '@/components/ui/status-bar';
import { EmptyState } from '@/components/ui/empty-state';
import { Separator } from '@/components/ui/separator';
import { Select, SelectTrigger, SelectValue, SelectContent, SelectItem } from '@/components/ui/select';
import { Avatar, AvatarImage, AvatarFallback } from '@/components/ui/avatar';
import { Alert, AlertTitle, AlertDescription } from '@/components/ui/alert';
import Modal from '../components/Modal';
import InlineEditableTitle from '../components/InlineEditableTitle';
import {
  ExclamationTriangleIcon,
  PencilIcon,
  ArrowPathIcon,
  ArrowTopRightOnSquareIcon,
  ClipboardDocumentIcon,
  SparklesIcon,
  XMarkIcon,
  CheckIcon,
  PlayIcon,
  DocumentTextIcon
} from '@heroicons/react/24/outline';
import { Spinner, SpinnerPage } from '@/components/ui/spinner';

export default function StyleGuide() {
  const [showModal, setShowModal] = useState(false);

  return (
    <div className="flex flex-col h-screen overflow-auto bg-background">
      {/* Header */}
      <div className="border-b border-border px-6 py-4">
        <h1 className="text-xl font-semibold">Kyomi Design System Style Guide</h1>
        <p className="text-sm text-muted-foreground">
          Reference: <code>docs/DESIGN_SYSTEM.md</code>
        </p>
      </div>

      {/* Content */}
      <div className="flex-1 p-6">
        <div className="max-w-7xl mx-auto space-y-8">

          {/* Logo Section */}
          <section>
            <h2 className="text-lg font-semibold mb-4">Brand Assets - Logo</h2>

            <div className="grid gap-6 md:grid-cols-2">
              {/* Full Logo */}
              <Card>
                <CardHeader>
                  <CardTitle className="text-base">Full Logo</CardTitle>
                  <CardDescription>Use in header, login pages, marketing</CardDescription>
                </CardHeader>
                <CardContent>
                  <div className="flex items-center justify-center p-6 bg-background border border-border rounded-md">
                    <img src="/kyomi_full_logo.svg" alt="Kyomi Full Logo" className="h-16" />
                  </div>
                  <div className="mt-4 space-y-1">
                    <p className="text-xs text-muted-foreground">
                      <span className="font-medium">File:</span> /kyomi_full_logo.svg
                    </p>
                    <p className="text-xs text-muted-foreground">
                      <span className="font-medium">Icon Color:</span> #d97706 (amber-600 gradient)
                    </p>
                    <p className="text-xs text-muted-foreground">
                      <span className="font-medium">Text Color:</span> #1f2937 (gray-800)
                    </p>
                  </div>
                </CardContent>
              </Card>

              {/* Small Logo */}
              <Card>
                <CardHeader>
                  <CardTitle className="text-base">Small Logo (Icon Only)</CardTitle>
                  <CardDescription>Use in sidebar, favicons, app icons</CardDescription>
                </CardHeader>
                <CardContent>
                  <div className="flex items-center justify-center p-6 bg-background border border-border rounded-md">
                    <img src="/kyomi_small_logo.svg" alt="Kyomi Icon" className="h-16 w-16" />
                  </div>
                  <div className="mt-4 space-y-1">
                    <p className="text-xs text-muted-foreground">
                      <span className="font-medium">File:</span> /kyomi_small_logo.svg
                    </p>
                    <p className="text-xs text-muted-foreground">
                      <span className="font-medium">Icon Color:</span> #d97706 (amber-600)
                    </p>
                  </div>
                </CardContent>
              </Card>
            </div>

            {/* White Logo (for dark backgrounds) */}
            <Card className="mt-4">
              <CardHeader>
                <CardTitle className="text-base">White Logo (Dark Backgrounds)</CardTitle>
                <CardDescription>Use on dark backgrounds, hero sections</CardDescription>
              </CardHeader>
              <CardContent>
                <div className="flex items-center justify-center p-6 bg-foreground rounded-md">
                  <img src="/kyomi_full_logo_white.svg" alt="Kyomi White Logo" className="h-16" />
                </div>
                <div className="mt-4">
                  <p className="text-xs text-muted-foreground">
                    <span className="font-medium">File:</span> /kyomi_full_logo_white.svg
                  </p>
                </div>
              </CardContent>
            </Card>
          </section>

          <Separator />

          {/* Icons Section */}
          <section>
            <h2 className="text-lg font-semibold mb-4">Icon System</h2>
            <p className="text-sm text-muted-foreground mb-6">
              We use <strong>Heroicons (24px outline)</strong> exclusively. Never mix icon libraries. All icons are imported from{' '}
              <code className="text-xs bg-muted px-1 rounded">@heroicons/react/24/outline</code>
            </p>

            <Card>
              <CardHeader>
                <CardTitle className="text-base">Standard Icons - Use These Throughout The App</CardTitle>
                <CardDescription>Consistent icons for common actions prevent UI confusion</CardDescription>
              </CardHeader>
              <CardContent>
                <div className="grid gap-6 md:grid-cols-2 lg:grid-cols-3">

                  {/* Warning/Alert */}
                  <div className="flex items-start gap-3">
                    <ExclamationTriangleIcon className="w-6 h-6 text-foreground flex-shrink-0" />
                    <div>
                      <p className="text-sm font-medium">ExclamationTriangleIcon</p>
                      <p className="text-xs text-muted-foreground">Warnings, alerts, errors</p>
                    </div>
                  </div>

                  {/* Edit */}
                  <div className="flex items-start gap-3">
                    <PencilIcon className="w-6 h-6 text-foreground flex-shrink-0" />
                    <div>
                      <p className="text-sm font-medium">PencilIcon</p>
                      <p className="text-xs text-muted-foreground">Edit, modify, rename</p>
                    </div>
                  </div>

                  {/* Refresh/Reload */}
                  <div className="flex items-start gap-3">
                    <ArrowPathIcon className="w-6 h-6 text-foreground flex-shrink-0" />
                    <div>
                      <p className="text-sm font-medium">ArrowPathIcon</p>
                      <p className="text-xs text-muted-foreground">Refresh, reload, retry</p>
                    </div>
                  </div>

                  {/* External Link */}
                  <div className="flex items-start gap-3">
                    <ArrowTopRightOnSquareIcon className="w-6 h-6 text-foreground flex-shrink-0" />
                    <div>
                      <p className="text-sm font-medium">ArrowTopRightOnSquareIcon</p>
                      <p className="text-xs text-muted-foreground">Open in new tab, external link</p>
                    </div>
                  </div>

                  {/* Copy */}
                  <div className="flex items-start gap-3">
                    <ClipboardDocumentIcon className="w-6 h-6 text-foreground flex-shrink-0" />
                    <div>
                      <p className="text-sm font-medium">ClipboardDocumentIcon</p>
                      <p className="text-xs text-muted-foreground">Copy to clipboard</p>
                    </div>
                  </div>

                  {/* AI/Magic */}
                  <div className="flex items-start gap-3">
                    <SparklesIcon className="w-6 h-6 text-foreground flex-shrink-0" />
                    <div>
                      <p className="text-sm font-medium">SparklesIcon</p>
                      <p className="text-xs text-muted-foreground">AI features, copilot, magic</p>
                    </div>
                  </div>

                  {/* Close/Cancel */}
                  <div className="flex items-start gap-3">
                    <XMarkIcon className="w-6 h-6 text-foreground flex-shrink-0" />
                    <div>
                      <p className="text-sm font-medium">XMarkIcon</p>
                      <p className="text-xs text-muted-foreground">Close, cancel, dismiss</p>
                    </div>
                  </div>

                  {/* Success/Check */}
                  <div className="flex items-start gap-3">
                    <CheckIcon className="w-6 h-6 text-foreground flex-shrink-0" />
                    <div>
                      <p className="text-sm font-medium">CheckIcon</p>
                      <p className="text-xs text-muted-foreground">Success, confirm, complete</p>
                    </div>
                  </div>

                  {/* Play/Execute */}
                  <div className="flex items-start gap-3">
                    <PlayIcon className="w-6 h-6 text-foreground flex-shrink-0" />
                    <div>
                      <p className="text-sm font-medium">PlayIcon</p>
                      <p className="text-xs text-muted-foreground">Run, execute, start</p>
                    </div>
                  </div>

                  {/* Document/File */}
                  <div className="flex items-start gap-3">
                    <DocumentTextIcon className="w-6 h-6 text-foreground flex-shrink-0" />
                    <div>
                      <p className="text-sm font-medium">DocumentTextIcon</p>
                      <p className="text-xs text-muted-foreground">Files, documents, text</p>
                    </div>
                  </div>

                </div>
              </CardContent>
            </Card>

            {/* Icon Usage Guidelines */}
            <Card className="mt-4">
              <CardHeader>
                <CardTitle className="text-base">Icon Usage Guidelines</CardTitle>
              </CardHeader>
              <CardContent className="space-y-4">
                <div>
                  <h4 className="text-sm font-semibold mb-2 flex items-center gap-2">
                    <CheckIcon className="w-4 h-4 text-success-foreground" />
                    <span>✅ DO</span>
                  </h4>
                  <ul className="text-sm text-muted-foreground space-y-1 ml-6">
                    <li>• Use outline variant (24px) consistently throughout the app</li>
                    <li>• Apply semantic colors: <code className="text-xs bg-muted px-1 rounded">text-foreground</code>, <code className="text-xs bg-muted px-1 rounded">text-muted-foreground</code>, <code className="text-xs bg-muted px-1 rounded">text-primary</code></li>
                    <li>• Use the same icon for the same action everywhere (e.g., always use PencilIcon for edit)</li>
                    <li>• Size icons with <code className="text-xs bg-muted px-1 rounded">w-5 h-5</code> (20px) or <code className="text-xs bg-muted px-1 rounded">w-6 h-6</code> (24px)</li>
                  </ul>
                </div>
                <div>
                  <h4 className="text-sm font-semibold mb-2 flex items-center gap-2">
                    <XMarkIcon className="w-4 h-4 text-error-foreground" />
                    <span>❌ DON'T</span>
                  </h4>
                  <ul className="text-sm text-muted-foreground space-y-1 ml-6">
                    <li>• Never mix solid and outline variants in the same context</li>
                    <li>• Never hardcode icon colors (e.g., <code className="text-xs bg-muted px-1 rounded line-through">text-blue-500</code>)</li>
                    <li>• Never use different icons for the same action (confuses users)</li>
                    <li>• Never import icons from other libraries (creates visual inconsistency)</li>
                  </ul>
                </div>
              </CardContent>
            </Card>
          </section>

          <Separator />

          {/* Spinners/Loading Section */}
          <section>
            <h2 className="text-lg font-semibold mb-4">Loading States</h2>
            <p className="text-sm text-muted-foreground mb-6">
              Use the standardized <code className="text-xs bg-muted px-1 rounded">Spinner</code> component from{' '}
              <code className="text-xs bg-muted px-1 rounded">@/components/ui/spinner</code>. For branded loading (OAuth, AI), use the animated Kyomi logo.
            </p>

            <div className="grid gap-4 md:grid-cols-2">
              {/* Spinner Sizes */}
              <Card>
                <CardHeader>
                  <CardTitle className="text-base">Spinner Sizes</CardTitle>
                  <CardDescription>Use appropriate size for context</CardDescription>
                </CardHeader>
                <CardContent>
                  <div className="space-y-4">
                    <div className="flex items-center gap-4">
                      <div className="w-16 flex justify-center"><Spinner size="xs" /></div>
                      <div>
                        <p className="text-sm font-medium">xs (12px)</p>
                        <p className="text-xs text-muted-foreground">Badges, inline text</p>
                      </div>
                    </div>
                    <div className="flex items-center gap-4">
                      <div className="w-16 flex justify-center"><Spinner size="sm" /></div>
                      <div>
                        <p className="text-sm font-medium">sm (16px) - Default</p>
                        <p className="text-xs text-muted-foreground">Buttons, inline loading</p>
                      </div>
                    </div>
                    <div className="flex items-center gap-4">
                      <div className="w-16 flex justify-center"><Spinner size="md" /></div>
                      <div>
                        <p className="text-sm font-medium">md (24px)</p>
                        <p className="text-xs text-muted-foreground">Cards, section loading</p>
                      </div>
                    </div>
                    <div className="flex items-center gap-4">
                      <div className="w-16 flex justify-center"><Spinner size="lg" className="text-muted-foreground" /></div>
                      <div>
                        <p className="text-sm font-medium">lg (32px)</p>
                        <p className="text-xs text-muted-foreground">Page sections</p>
                      </div>
                    </div>
                    <div className="flex items-center gap-4">
                      <div className="w-16 flex justify-center"><Spinner size="xl" className="text-muted-foreground" /></div>
                      <div>
                        <p className="text-sm font-medium">xl (48px)</p>
                        <p className="text-xs text-muted-foreground">Full page loading</p>
                      </div>
                    </div>
                  </div>
                </CardContent>
              </Card>

              {/* Branded Loading */}
              <Card>
                <CardHeader>
                  <CardTitle className="text-base">Branded Loading</CardTitle>
                  <CardDescription>Use animated Kyomi logo for branded moments</CardDescription>
                </CardHeader>
                <CardContent>
                  <div className="space-y-4">
                    <div className="flex items-center gap-4">
                      <div className="w-16 flex justify-center">
                        <img src="/kyomi_animated_logo.svg" alt="Loading" className="w-8 h-8" />
                      </div>
                      <div>
                        <p className="text-sm font-medium">Animated Kyomi Logo</p>
                        <p className="text-xs text-muted-foreground">OAuth flows, AI thinking, dashboard loading</p>
                      </div>
                    </div>
                  </div>
                  <div className="mt-6 p-4 bg-muted rounded-md">
                    <p className="text-xs font-medium mb-2">Usage:</p>
                    <code className="text-xs text-muted-foreground">
                      {'<img src="/kyomi_animated_logo.svg" className="w-8 h-8" />'}
                    </code>
                  </div>
                </CardContent>
              </Card>
            </div>

            {/* Usage Examples */}
            <Card className="mt-4">
              <CardHeader>
                <CardTitle className="text-base">Usage Examples</CardTitle>
              </CardHeader>
              <CardContent className="space-y-4">
                <div className="flex items-center gap-4">
                  <Button disabled>
                    <Spinner className="mr-2" />
                    Saving...
                  </Button>
                  <code className="text-xs text-muted-foreground">{'<Spinner className="mr-2" />'}</code>
                </div>
                <div className="flex items-center gap-4">
                  <Badge variant="secondary" className="gap-1">
                    <Spinner size="xs" />
                    Loading
                  </Badge>
                  <code className="text-xs text-muted-foreground">{'<Spinner size="xs" />'}</code>
                </div>
                <div className="flex items-center gap-4">
                  <div className="p-4 border rounded-md">
                    <Spinner size="md" className="text-muted-foreground" />
                  </div>
                  <code className="text-xs text-muted-foreground">{'<Spinner size="md" className="text-muted-foreground" />'}</code>
                </div>
              </CardContent>
            </Card>
          </section>

          <Separator />

          {/* Colors Section */}
          <section>
            <h2 className="text-lg font-semibold mb-4">Color System</h2>
            <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
              {/* Primary */}
              <Card>
                <CardHeader>
                  <CardTitle className="text-base">Primary</CardTitle>
                  <CardDescription>Main brand color</CardDescription>
                </CardHeader>
                <CardContent>
                  <div className="flex gap-2">
                    <div className="w-16 h-16 bg-primary rounded-md shadow" />
                    <div className="flex-1">
                      <p className="text-xs text-muted-foreground">bg-primary</p>
                      <p className="text-xs text-muted-foreground">text-primary</p>
                    </div>
                  </div>
                </CardContent>
              </Card>

              {/* Secondary */}
              <Card>
                <CardHeader>
                  <CardTitle className="text-base">Secondary</CardTitle>
                  <CardDescription>Secondary actions</CardDescription>
                </CardHeader>
                <CardContent>
                  <div className="flex gap-2">
                    <div className="w-16 h-16 bg-secondary rounded-md shadow" />
                    <div className="flex-1">
                      <p className="text-xs text-muted-foreground">bg-secondary</p>
                      <p className="text-xs text-muted-foreground">text-secondary</p>
                    </div>
                  </div>
                </CardContent>
              </Card>

              {/* Accent */}
              <Card>
                <CardHeader>
                  <CardTitle className="text-base">Accent</CardTitle>
                  <CardDescription>Hover states</CardDescription>
                </CardHeader>
                <CardContent>
                  <div className="flex gap-2">
                    <div className="w-16 h-16 bg-accent border border-border rounded-md shadow" />
                    <div className="flex-1">
                      <p className="text-xs text-muted-foreground">bg-accent</p>
                      <p className="text-xs text-muted-foreground">text-accent-foreground</p>
                    </div>
                  </div>
                </CardContent>
              </Card>

              {/* Destructive */}
              <Card>
                <CardHeader>
                  <CardTitle className="text-base">Destructive</CardTitle>
                  <CardDescription>Dangerous actions</CardDescription>
                </CardHeader>
                <CardContent>
                  <div className="flex gap-2">
                    <div className="w-16 h-16 bg-destructive rounded-md shadow" />
                    <div className="flex-1">
                      <p className="text-xs text-muted-foreground">bg-destructive</p>
                      <p className="text-xs text-muted-foreground">text-destructive-foreground</p>
                    </div>
                  </div>
                </CardContent>
              </Card>

              {/* Muted */}
              <Card>
                <CardHeader>
                  <CardTitle className="text-base">Muted</CardTitle>
                  <CardDescription>Disabled states</CardDescription>
                </CardHeader>
                <CardContent>
                  <div className="flex gap-2">
                    <div className="w-16 h-16 bg-muted border border-border rounded-md shadow" />
                    <div className="flex-1">
                      <p className="text-xs text-muted-foreground">bg-muted</p>
                      <p className="text-xs text-muted-foreground">text-muted-foreground</p>
                    </div>
                  </div>
                </CardContent>
              </Card>

              {/* Border */}
              <Card>
                <CardHeader>
                  <CardTitle className="text-base">Border</CardTitle>
                  <CardDescription>Default borders</CardDescription>
                </CardHeader>
                <CardContent>
                  <div className="flex gap-2">
                    <div className="w-16 h-16 bg-background border-4 border-border rounded-md shadow" />
                    <div className="flex-1">
                      <p className="text-xs text-muted-foreground">border-border</p>
                      <p className="text-xs text-muted-foreground">border-input</p>
                    </div>
                  </div>
                </CardContent>
              </Card>
            </div>

            {/* Active Status Colors */}
            <h3 className="text-base font-semibold mt-6 mb-3">Status Colors</h3>
            <p className="text-sm text-muted-foreground mb-4">Standard status colors used throughout the application</p>

            <div className="grid gap-2 md:grid-cols-4">
              <div className="bg-warning border border-warning-border text-warning-foreground p-3 rounded-md">
                <p className="text-xs font-medium">⚠️ Warning</p>
                <p className="text-xs opacity-75 mt-1">Muted gold</p>
              </div>
              <div className="bg-error border border-error-border text-error-foreground p-3 rounded-md">
                <p className="text-xs font-medium">❌ Error</p>
                <p className="text-xs opacity-75 mt-1">Coral red</p>
              </div>
              <div className="bg-success border border-success-border text-success-foreground p-3 rounded-md">
                <p className="text-xs font-medium">✅ Success</p>
                <p className="text-xs opacity-75 mt-1">Teal green</p>
              </div>
              <div className="bg-info border border-info-border text-info-foreground p-3 rounded-md">
                <p className="text-xs font-medium">ℹ️ Info</p>
                <p className="text-xs opacity-75 mt-1">Royal blue</p>
              </div>
            </div>
          </section>

          <Separator />

          {/* Typography Section */}
          <section>
            <h2 className="text-lg font-semibold mb-4">Typography</h2>
            <Card>
              <CardContent className="pt-6 space-y-4">
                <div>
                  <p className="text-xl font-semibold">Heading XL (text-xl font-semibold)</p>
                  <code className="text-xs text-muted-foreground">Page titles, modal titles</code>
                </div>
                <div>
                  <p className="text-lg font-semibold">Heading LG (text-lg font-semibold)</p>
                  <code className="text-xs text-muted-foreground">Section titles, card titles</code>
                </div>
                <div>
                  <p className="text-base font-normal">Body Text (text-base font-normal)</p>
                  <code className="text-xs text-muted-foreground">Paragraphs, descriptions</code>
                </div>
                <div>
                  <p className="text-sm font-medium">Label Text (text-sm font-medium)</p>
                  <code className="text-xs text-muted-foreground">Form labels, nav items</code>
                </div>
                <div>
                  <p className="text-sm text-muted-foreground">Caption Text (text-sm text-muted-foreground)</p>
                  <code className="text-xs text-muted-foreground">Helper text, secondary info</code>
                </div>
                <div>
                  <p className="text-xs">Small Text (text-xs)</p>
                  <code className="text-xs text-muted-foreground">Badges, timestamps, metadata</code>
                </div>
              </CardContent>
            </Card>
          </section>

          <Separator />

          {/* Buttons Section */}
          <section>
            <h2 className="text-lg font-semibold mb-4">Buttons</h2>
            <Card>
              <CardContent className="pt-6">
                <div className="space-y-6">
                  {/* Variants */}
                  <div>
                    <h3 className="text-base font-semibold mb-3">Variants</h3>
                    <div className="flex flex-wrap gap-2">
                      <Button variant="default">Default</Button>
                      <Button variant="secondary">Secondary</Button>
                      <Button variant="outline">Outline</Button>
                      <Button variant="ghost">Ghost</Button>
                      <Button variant="destructive">Destructive</Button>
                      <Button variant="link">Link</Button>
                    </div>
                  </div>

                  {/* Sizes */}
                  <div>
                    <h3 className="text-base font-semibold mb-3">Sizes</h3>
                    <div className="flex flex-wrap items-center gap-2">
                      <Button size="sm">Small</Button>
                      <Button size="default">Default</Button>
                      <Button size="lg">Large</Button>
                      <Button size="icon">
                        <svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                          <line x1="12" y1="5" x2="12" y2="19"></line>
                          <line x1="5" y1="12" x2="19" y2="12"></line>
                        </svg>
                      </Button>
                    </div>
                  </div>

                  {/* Disabled */}
                  <div>
                    <h3 className="text-base font-semibold mb-3">Disabled State</h3>
                    <div className="flex flex-wrap gap-2">
                      <Button variant="default" disabled>Disabled</Button>
                      <Button variant="secondary" disabled>Disabled</Button>
                      <Button variant="outline" disabled>Disabled</Button>
                    </div>
                  </div>
                </div>
              </CardContent>
            </Card>
          </section>

          <Separator />

          {/* Form Inputs Section */}
          <section>
            <h2 className="text-lg font-semibold mb-4">Form Inputs</h2>
            <p className="text-sm text-muted-foreground mb-4">
              All form inputs use semantic border tokens (<code>border-border</code>, <code>border-input</code>)
              and never hardcode colors. Always use the design system components (Input, Select)
              rather than native HTML elements to ensure consistency.
            </p>
            <Card>
              <CardContent className="pt-6 space-y-4">
                {/* Text Input */}
                <div>
                  <Label htmlFor="demo-input">Text Input</Label>
                  <Input id="demo-input" type="text" placeholder="Enter text..." />
                </div>

                {/* Email Input */}
                <div>
                  <Label htmlFor="demo-email">Email Input</Label>
                  <Input id="demo-email" type="email" placeholder="you@example.com" />
                </div>

                {/* Disabled Input */}
                <div>
                  <Label htmlFor="demo-disabled">Disabled Input</Label>
                  <Input id="demo-disabled" type="text" placeholder="Disabled..." disabled />
                </div>

                {/* Select */}
                <div>
                  <Label htmlFor="demo-select">Select Dropdown</Label>
                  <Select>
                    <SelectTrigger>
                      <SelectValue placeholder="Select an option" />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="option1">Option 1</SelectItem>
                      <SelectItem value="option2">Option 2</SelectItem>
                      <SelectItem value="option3">Option 3</SelectItem>
                    </SelectContent>
                  </Select>
                  <p className="text-xs text-muted-foreground mt-1">
                    ⚠️ Always use <code>&lt;Select&gt;</code> component, never native <code>&lt;select&gt;</code>
                  </p>
                </div>

                {/* Inline Editable Title */}
                <div>
                  <Label>InlineEditableTitle Component</Label>
                  <p className="text-xs text-muted-foreground mb-2">
                    For titles/headings that are editable inline. Click to edit, shows pencil icon on hover, save/cancel buttons appear when editing.
                  </p>
                  <InlineEditableTitle
                    value="Dashboard Title"
                    onSave={(newTitle) => {}}
                    placeholder="Click to edit..."
                  />
                  <p className="text-xs text-muted-foreground mt-2">
                    <strong>Usage:</strong> <code className="text-xs">{'<InlineEditableTitle value={title} onSave={setTitle} placeholder="..." />'}</code>
                  </p>
                  <p className="text-xs text-muted-foreground mt-1">
                    ⚠️ Use this component for all inline-editable titles (Chat, Dashboard, etc.) to ensure consistency
                  </p>
                </div>
              </CardContent>
            </Card>
          </section>

          <Separator />

          {/* Badges Section */}
          <section>
            <h2 className="text-lg font-semibold mb-4">Badges</h2>
            <Card>
              <CardContent className="pt-6">
                <div className="flex flex-wrap gap-2">
                  <Badge variant="default">Default</Badge>
                  <Badge variant="secondary">Secondary</Badge>
                  <Badge variant="destructive">Destructive</Badge>
                  <Badge variant="outline">Outline</Badge>
                </div>
              </CardContent>
            </Card>
          </section>

          <Separator />

          {/* Status Badges Section */}
          <section>
            <h2 className="text-lg font-semibold mb-4">Status Badges</h2>
            <p className="text-sm text-muted-foreground mb-4">
              Status badges for inline status indicators. Use semantic variants that map to design system status colors.
            </p>
            <Card>
              <CardHeader>
                <CardTitle className="text-sm">Status Badge Variants</CardTitle>
                <CardDescription>All variants use semantic tokens from index.css</CardDescription>
              </CardHeader>
              <CardContent>
                <div className="flex flex-wrap gap-2">
                  <StatusBadge variant="default">Default</StatusBadge>
                  <StatusBadge variant="success">Success</StatusBadge>
                  <StatusBadge variant="warning">Warning</StatusBadge>
                  <StatusBadge variant="error">Error</StatusBadge>
                  <StatusBadge variant="info">Info</StatusBadge>
                </div>
                <div className="mt-4">
                  <p className="text-xs text-muted-foreground mb-2"><strong>Usage:</strong></p>
                  <pre className="bg-muted p-2 rounded text-xs overflow-x-auto">
{`import { StatusBadge } from '@/components/ui/status-badge';

<StatusBadge variant="success">Authenticated</StatusBadge>
<StatusBadge variant="warning">Expires Soon</StatusBadge>
<StatusBadge variant="error">Failed</StatusBadge>`}
                  </pre>
                </div>
              </CardContent>
            </Card>
          </section>

          <Separator />

          {/* Status Bars Section */}
          <section>
            <h2 className="text-lg font-semibold mb-4">Status Bars</h2>
            <p className="text-sm text-muted-foreground mb-4">
              Prominent status bars for important notifications, typically positioned at top or bottom of layouts.
            </p>

            <div className="space-y-4">
              <Card>
                <CardHeader>
                  <CardTitle className="text-sm">Status Bar Variants</CardTitle>
                  <CardDescription>Full-width bars with semantic status colors</CardDescription>
                </CardHeader>
                <CardContent className="space-y-3">
                  <StatusBar variant="success">
                    <StatusBarText variant="success">✓ OAuth connection successful</StatusBarText>
                  </StatusBar>

                  <StatusBar variant="warning">
                    <StatusBarText variant="warning">⚠️ Your OAuth token expires in 5 minutes</StatusBarText>
                    <Button size="sm" variant="outline">Reconnect</Button>
                  </StatusBar>

                  <StatusBar variant="error">
                    <StatusBarText variant="error">✗ OAuth token expired - reconnection required</StatusBarText>
                    <Button size="sm" variant="outline">Reconnect Now</Button>
                  </StatusBar>

                  <StatusBar variant="info">
                    <StatusBarText variant="info">ℹ️ BigQuery catalog is refreshing in the background</StatusBarText>
                  </StatusBar>
                </CardContent>
              </Card>

              <Card>
                <CardHeader>
                  <CardTitle className="text-sm">Usage Example</CardTitle>
                </CardHeader>
                <CardContent>
                  <pre className="bg-muted p-3 rounded text-xs overflow-x-auto">
{`import { StatusBar, StatusBarText } from '@/components/ui/status-bar';
import { Button } from '@/components/ui/button';

<StatusBar variant="warning">
  <StatusBarText variant="warning">
    Token expires in 5 minutes
  </StatusBarText>
  <Button size="sm" variant="outline" onClick={handleReconnect}>
    Reconnect
  </Button>
</StatusBar>`}
                  </pre>
                </CardContent>
              </Card>
            </div>
          </section>

          <Separator />

          {/* Empty States Section */}
          <section>
            <h2 className="text-lg font-semibold mb-4">Empty States</h2>
            <p className="text-sm text-muted-foreground mb-4">
              Empty state components for when there's no data to display. Includes optional icon, title, description, and action button.
            </p>

            <div className="space-y-4">
              <Card>
                <CardHeader>
                  <CardTitle className="text-sm">Empty State Variants</CardTitle>
                  <CardDescription>Different semantic variants for various contexts</CardDescription>
                </CardHeader>
                <CardContent className="space-y-4">
                  <EmptyState
                    variant="default"
                    icon={
                      <svg className="w-12 h-12" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M9 12h6m-6 4h6m2 5H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z" />
                      </svg>
                    }
                    title="No charts yet"
                    description="Create your first chart to get started with data visualization"
                    action={<Button size="sm" variant="default">Create Chart</Button>}
                  />

                  <EmptyState
                    variant="info"
                    icon={
                      <svg className="w-12 h-12" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M13 16h-1v-4h-1m1-4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
                      </svg>
                    }
                    title="No results found"
                    description="Try adjusting your search or filter criteria"
                  />

                  <EmptyState
                    variant="warning"
                    icon={
                      <svg className="w-12 h-12" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z" />
                      </svg>
                    }
                    title="Data unavailable"
                    description="The requested data could not be loaded at this time"
                  />
                </CardContent>
              </Card>

              <Card>
                <CardHeader>
                  <CardTitle className="text-sm">Usage Example</CardTitle>
                </CardHeader>
                <CardContent>
                  <pre className="bg-muted p-3 rounded text-xs overflow-x-auto">
{`import { EmptyState } from '@/components/ui/empty-state';
import { Button } from '@/components/ui/button';

<EmptyState
  variant="default"
  icon={<ChartIcon />}
  title="No charts yet"
  description="Create your first chart to get started"
  action={<Button onClick={onCreate}>Create Chart</Button>}
/>`}
                  </pre>
                </CardContent>
              </Card>
            </div>
          </section>

          <Separator />

          {/* Tooltips Section */}
          <section>
            <h2 className="text-lg font-semibold mb-4">Tooltips</h2>
            <p className="text-sm text-muted-foreground mb-4">
              Tooltips provide contextual help on hover. Use the standard neutral style across all UI components, charts, and interactive elements.
            </p>

            <Card>
              <CardHeader>
                <CardTitle className="text-sm">Standard Tooltip Style</CardTitle>
                <CardDescription>Neutral, readable tooltips for both UI and data visualizations</CardDescription>
              </CardHeader>
              <CardContent>
                <div className="space-y-4">
                  <div>
                    <p className="text-sm font-medium mb-2">Visual Examples:</p>
                    <div className="inline-flex items-center gap-4">
                      <div
                        className="px-3 py-2 bg-white text-foreground border border-border rounded-md shadow-md text-xs"
                        style={{ pointerEvents: 'none' }}
                      >
                        Standard tooltip content
                      </div>
                      <div
                        className="px-3 py-2 bg-white text-foreground border border-border rounded-md shadow-md text-xs"
                        style={{ pointerEvents: 'none' }}
                      >
                        Revenue: $42,500<br/>
                        Growth: +12.5%
                      </div>
                    </div>
                  </div>

                  <div className="space-y-2">
                    <p className="text-sm font-medium">Implementation (Pick One):</p>

                    <div className="border border-border rounded-md p-3 space-y-2">
                      <p className="text-xs font-semibold text-foreground">1. React Components (Recommended)</p>
                      <p className="text-xs text-muted-foreground">Use the Radix Tooltip component for all React/UI tooltips:</p>
                      <pre className="bg-muted p-2 rounded text-xs overflow-x-auto">
{`import { Tooltip, TooltipTrigger, TooltipContent } from '@/components/ui/tooltip';

<Tooltip>
  <TooltipTrigger>Hover me</TooltipTrigger>
  <TooltipContent>Tooltip text</TooltipContent>
</Tooltip>`}
                      </pre>
                      <p className="text-xs text-muted-foreground mt-2">
                        <strong>Location:</strong> <code className="bg-background px-1 rounded">components/ui/tooltip.jsx</code>
                      </p>
                    </div>

                    <div className="border border-border rounded-md p-3 space-y-2">
                      <p className="text-xs font-semibold text-foreground">2. Charts & D3 Visualizations</p>
                      <p className="text-xs text-muted-foreground">Use the <code className="bg-background px-1 rounded">.chart-tooltip</code> CSS class:</p>
                      <pre className="bg-muted p-2 rounded text-xs overflow-x-auto">
{`const tooltip = d3.select(container)
  .append('div')
  .attr('class', 'chart-tooltip');`}
                      </pre>
                      <p className="text-xs text-muted-foreground mt-2">
                        <strong>Location:</strong> <code className="bg-background px-1 rounded">index.css</code> (@layer components)
                      </p>
                    </div>

                    <div className="border border-border rounded-md p-3 space-y-2">
                      <p className="text-xs font-semibold text-foreground">3. Simple Text Tooltips</p>
                      <p className="text-xs text-muted-foreground">For basic text-only tooltips, native HTML title attribute is acceptable:</p>
                      <pre className="bg-muted p-2 rounded text-xs overflow-x-auto">
{`<button title="Close window">×</button>`}
                      </pre>
                      <p className="text-xs text-muted-foreground mt-2">
                        Note: Browser-styled, limited formatting
                      </p>
                    </div>
                  </div>

                  <div className="p-3 bg-info border border-info-border rounded-md">
                    <p className="text-xs text-info-foreground">
                      <strong>Single Source of Truth:</strong> Tooltip styling is defined once in the Tooltip component and .chart-tooltip class.
                      Never duplicate these styles inline. This ensures consistency and makes global changes easy.
                    </p>
                  </div>
                </div>
              </CardContent>
            </Card>
          </section>

          <Separator />

          {/* Cards Section */}
          <section>
            <h2 className="text-lg font-semibold mb-4">Cards</h2>
            <div className="grid gap-4 md:grid-cols-2">
              <Card>
                <CardHeader>
                  <CardTitle>Card Title</CardTitle>
                  <CardDescription>Card description or subtitle</CardDescription>
                </CardHeader>
                <CardContent>
                  <p className="text-sm">This is the card content area. Use CardContent for the main content.</p>
                </CardContent>
              </Card>

              <Card>
                <CardHeader>
                  <CardTitle>Another Card</CardTitle>
                  <CardDescription>With more content</CardDescription>
                </CardHeader>
                <CardContent>
                  <p className="text-sm">Cards provide consistent spacing and styling for content containers.</p>
                </CardContent>
              </Card>
            </div>
          </section>

          <Separator />

          {/* Avatars Section */}
          <section>
            <h2 className="text-lg font-semibold mb-4">Avatars</h2>
            <Card>
              <CardContent className="pt-6">
                <div className="flex gap-4">
                  <Avatar>
                    <AvatarImage src="https://api.dicebear.com/7.x/avataaars/svg?seed=John" alt="User" />
                    <AvatarFallback>JD</AvatarFallback>
                  </Avatar>
                  <Avatar>
                    <AvatarFallback>AB</AvatarFallback>
                  </Avatar>
                  <Avatar>
                    <AvatarFallback className="bg-primary text-primary-foreground">CD</AvatarFallback>
                  </Avatar>
                </div>
              </CardContent>
            </Card>
          </section>

          <Separator />

          {/* Spacing Section */}
          <section>
            <h2 className="text-lg font-semibold mb-4">Spacing Scale</h2>
            <Card>
              <CardContent className="pt-6 space-y-2">
                <div className="flex items-center gap-4">
                  <div className="w-1 h-6 bg-primary" />
                  <code className="text-xs">1 = 0.25rem (4px)</code>
                </div>
                <div className="flex items-center gap-4">
                  <div className="w-2 h-6 bg-primary" />
                  <code className="text-xs">2 = 0.5rem (8px)</code>
                </div>
                <div className="flex items-center gap-4">
                  <div className="w-3 h-6 bg-primary" />
                  <code className="text-xs">3 = 0.75rem (12px)</code>
                </div>
                <div className="flex items-center gap-4">
                  <div className="w-4 h-6 bg-primary" />
                  <code className="text-xs">4 = 1rem (16px)</code>
                </div>
                <div className="flex items-center gap-4">
                  <div className="w-6 h-6 bg-primary" />
                  <code className="text-xs">6 = 1.5rem (24px)</code>
                </div>
                <div className="flex items-center gap-4">
                  <div className="w-8 h-6 bg-primary" />
                  <code className="text-xs">8 = 2rem (32px)</code>
                </div>
                <div className="flex items-center gap-4">
                  <div className="w-12 h-6 bg-primary" />
                  <code className="text-xs">12 = 3rem (48px)</code>
                </div>
                <div className="flex items-center gap-4">
                  <div className="w-16 h-6 bg-primary" />
                  <code className="text-xs">16 = 4rem (64px)</code>
                </div>
              </CardContent>
            </Card>
          </section>

          <Separator />

          {/* Borders & Shadows Section */}
          <section>
            <h2 className="text-lg font-semibold mb-4">Borders & Shadows</h2>
            <div className="grid gap-4 md:grid-cols-2">
              <Card>
                <CardHeader>
                  <CardTitle className="text-base">Border Radius</CardTitle>
                </CardHeader>
                <CardContent className="space-y-3">
                  <div className="flex items-center gap-4">
                    <div className="w-16 h-16 bg-accent rounded-md border border-border" />
                    <code className="text-xs">rounded-md (4px)</code>
                  </div>
                  <div className="flex items-center gap-4">
                    <div className="w-16 h-16 bg-accent rounded-lg border border-border" />
                    <code className="text-xs">rounded-lg (8px)</code>
                  </div>
                  <div className="flex items-center gap-4">
                    <div className="w-16 h-16 bg-accent rounded-xl border border-border" />
                    <code className="text-xs">rounded-xl (12px)</code>
                  </div>
                  <div className="flex items-center gap-4">
                    <div className="w-16 h-16 bg-accent rounded-full border border-border" />
                    <code className="text-xs">rounded-full (circle)</code>
                  </div>
                </CardContent>
              </Card>

              <Card>
                <CardHeader>
                  <CardTitle className="text-base">Shadow Levels</CardTitle>
                </CardHeader>
                <CardContent className="space-y-3">
                  <div className="flex items-center gap-4">
                    <div className="w-16 h-16 bg-background rounded-md shadow-sm border border-border" />
                    <code className="text-xs">shadow-sm</code>
                  </div>
                  <div className="flex items-center gap-4">
                    <div className="w-16 h-16 bg-background rounded-md shadow border border-border" />
                    <code className="text-xs">shadow</code>
                  </div>
                  <div className="flex items-center gap-4">
                    <div className="w-16 h-16 bg-background rounded-md shadow-lg border border-border" />
                    <code className="text-xs">shadow-lg</code>
                  </div>
                  <div className="flex items-center gap-4">
                    <div className="w-16 h-16 bg-background rounded-md shadow-xl border border-border" />
                    <code className="text-xs">shadow-xl</code>
                  </div>
                </CardContent>
              </Card>
            </div>
          </section>

          <Separator />

          {/* Modals Section */}
          <section>
            <h2 className="text-lg font-semibold mb-4">Modals</h2>
            <p className="text-sm text-muted-foreground mb-4">
              Center overlays with 50% black backdrop for focused tasks
            </p>
            <Card>
              <CardContent className="pt-6">
                <Button variant="outline" onClick={() => setShowModal(true)}>
                  Open Modal Example
                </Button>
              </CardContent>
            </Card>
          </section>

          <Separator />

          {/* Alerts Section */}
          <section>
            <h2 className="text-lg font-semibold mb-4">Alerts (Inline)</h2>
            <div className="space-y-3">
              <Alert variant="default">
                <AlertTitle>Default Alert</AlertTitle>
                <AlertDescription>This is a default alert message with neutral styling.</AlertDescription>
              </Alert>
              <Alert variant="info">
                <AlertTitle>Info Alert</AlertTitle>
                <AlertDescription>Informational message for the user.</AlertDescription>
              </Alert>
              <Alert variant="warning">
                <AlertTitle>Warning Alert</AlertTitle>
                <AlertDescription>Warning message that requires attention.</AlertDescription>
              </Alert>
              <Alert variant="error">
                <AlertTitle>Error Alert</AlertTitle>
                <AlertDescription>Error message indicating something went wrong.</AlertDescription>
              </Alert>
              <Alert variant="success">
                <AlertTitle>Success Alert</AlertTitle>
                <AlertDescription>Success message confirming an action.</AlertDescription>
              </Alert>
            </div>
          </section>

          <Separator />

          {/* Toast Notifications Mockup */}
          <section>
            <h2 className="text-lg font-semibold mb-4">Toast Notifications (Proposed)</h2>
            <p className="text-sm text-muted-foreground mb-4">
              Non-blocking notifications that appear in top-right corner and auto-dismiss
            </p>
            <Card>
              <CardContent className="pt-6">
                <div className="space-y-3 max-w-sm">
                  {/* Success Toast */}
                  <div className="bg-success border border-success-border rounded-lg shadow-lg p-4 flex items-start gap-3">
                    <CheckIcon className="w-5 h-5 text-success-foreground flex-shrink-0 mt-0.5" />
                    <div className="flex-1">
                      <p className="text-sm font-medium text-success-foreground">Dashboard saved successfully!</p>
                    </div>
                    <button className="text-success-foreground/60 hover:text-success-foreground">
                      <XMarkIcon className="w-4 h-4" />
                    </button>
                  </div>

                  {/* Error Toast */}
                  <div className="bg-error border border-error-border rounded-lg shadow-lg p-4 flex items-start gap-3">
                    <ExclamationTriangleIcon className="w-5 h-5 text-error-foreground flex-shrink-0 mt-0.5" />
                    <div className="flex-1">
                      <p className="text-sm font-medium text-error-foreground">Failed to connect to BigQuery</p>
                    </div>
                    <button className="text-error-foreground/60 hover:text-error-foreground">
                      <XMarkIcon className="w-4 h-4" />
                    </button>
                  </div>

                  {/* Warning Toast */}
                  <div className="bg-warning border border-warning-border rounded-lg shadow-lg p-4 flex items-start gap-3">
                    <ExclamationTriangleIcon className="w-5 h-5 text-warning-foreground flex-shrink-0 mt-0.5" />
                    <div className="flex-1">
                      <p className="text-sm font-medium text-warning-foreground">Session expires in 5 minutes</p>
                    </div>
                    <button className="text-warning-foreground/60 hover:text-warning-foreground">
                      <XMarkIcon className="w-4 h-4" />
                    </button>
                  </div>

                  {/* Info Toast */}
                  <div className="bg-info border border-info-border rounded-lg shadow-lg p-4 flex items-start gap-3">
                    <svg className="w-5 h-5 text-info-foreground flex-shrink-0 mt-0.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                      <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M13 16h-1v-4h-1m1-4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
                    </svg>
                    <div className="flex-1">
                      <p className="text-sm font-medium text-info-foreground">Catalog is refreshing in the background</p>
                    </div>
                    <button className="text-info-foreground/60 hover:text-info-foreground">
                      <XMarkIcon className="w-4 h-4" />
                    </button>
                  </div>
                </div>
              </CardContent>
            </Card>
          </section>

          <Separator />

          {/* ConfirmDialog Mockup */}
          <section>
            <h2 className="text-lg font-semibold mb-4">Confirm Dialog (Proposed)</h2>
            <p className="text-sm text-muted-foreground mb-4">
              Replaces browser confirm() with styled React component
            </p>
            <div className="grid gap-4 md:grid-cols-2">
              {/* Destructive Confirmation */}
              <Card>
                <CardHeader>
                  <CardTitle className="text-sm">Destructive Action</CardTitle>
                </CardHeader>
                <CardContent>
                  <div className="bg-muted p-6 rounded-lg border border-border">
                    <div className="bg-background rounded-lg shadow-xl max-w-md mx-auto p-6">
                      <div className="flex items-start gap-3 mb-4">
                        <div className="p-2 bg-destructive/10 rounded-lg">
                          <ExclamationTriangleIcon className="w-6 h-6 text-destructive" />
                        </div>
                        <div className="flex-1">
                          <h3 className="text-base font-semibold text-foreground mb-1">Delete Chat?</h3>
                          <p className="text-sm text-muted-foreground">
                            This action cannot be undone. All messages will be permanently deleted.
                          </p>
                        </div>
                      </div>
                      <div className="flex gap-2 justify-end">
                        <Button variant="outline" size="sm">Cancel</Button>
                        <Button variant="destructive" size="sm">Delete</Button>
                      </div>
                    </div>
                  </div>
                </CardContent>
              </Card>

              {/* Default Confirmation */}
              <Card>
                <CardHeader>
                  <CardTitle className="text-sm">Default Action</CardTitle>
                </CardHeader>
                <CardContent>
                  <div className="bg-muted p-6 rounded-lg border border-border">
                    <div className="bg-background rounded-lg shadow-xl max-w-md mx-auto p-6">
                      <div className="flex items-start gap-3 mb-4">
                        <div className="p-2 bg-primary/10 rounded-lg">
                          <svg className="w-6 h-6 text-primary" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M13 16h-1v-4h-1m1-4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
                          </svg>
                        </div>
                        <div className="flex-1">
                          <h3 className="text-base font-semibold text-foreground mb-1">Unsaved Changes</h3>
                          <p className="text-sm text-muted-foreground">
                            You have unsaved changes. Do you want to continue?
                          </p>
                        </div>
                      </div>
                      <div className="flex gap-2 justify-end">
                        <Button variant="outline" size="sm">Cancel</Button>
                        <Button variant="default" size="sm">Continue</Button>
                      </div>
                    </div>
                  </div>
                </CardContent>
              </Card>
            </div>
          </section>

        </div>
      </div>

      {/* Modal Example */}
      <Modal
        show={showModal}
        onClose={() => setShowModal(false)}
        title="Example Modal"
        size="lg"
        footer={
          <>
            <Button variant="outline" onClick={() => setShowModal(false)}>
              Cancel
            </Button>
            <Button variant="default" onClick={() => setShowModal(false)}>
              Save Changes
            </Button>
          </>
        }
      >
        <div className="space-y-4">
          <p className="text-sm text-muted-foreground">
            This is an example modal with a 50% black backdrop. Notice how the backdrop helps you focus on the modal content.
          </p>
          <div>
            <Label htmlFor="modal-input">Example Input</Label>
            <Input id="modal-input" placeholder="Enter something..." />
          </div>
          <div>
            <Label htmlFor="modal-select">Example Select</Label>
            <Select>
              <SelectTrigger>
                <SelectValue placeholder="Choose an option" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="option1">Option 1</SelectItem>
                <SelectItem value="option2">Option 2</SelectItem>
                <SelectItem value="option3">Option 3</SelectItem>
              </SelectContent>
            </Select>
          </div>
        </div>
      </Modal>
    </div>
  );
}
