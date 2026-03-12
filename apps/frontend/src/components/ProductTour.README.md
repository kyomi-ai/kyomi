# Product Tour System

Multi-tour onboarding system that shows contextual help when users first encounter features.

## How It Works

Each tour:
- Shows **once** when user first encounters a feature
- Tracked in **backend database** (persists across browsers and devices)
- Tours disabled if backend is unavailable (no localStorage fallback)
- Highlights specific UI elements with spotlight overlay
- Clean, minimal tooltips explain what to do

## Available Tours

1. **firstChart** - Shows after first chat with a chart (pin + save to dashboard)
2. **agentThinking** - Shows when agent thinking section first appears (click to expand)
3. **sqlEditor** - Shows when SQL editor is first loaded (query history)
4. **dashboardEditor** - Shows when dashboard editor is first opened (view mode toggle)
5. **dashboardChartEdit** - Shows when first chart renders in dashboard editor preview (edit button)
6. **chartCopilot** - Shows when chart builder modal opens (AI copilot input)

## Adding a New Tour

### 1. Define the Tour in ProductTour.jsx

Add to the `TOURS` object:

```javascript
const TOURS = {
  // ... existing tours

  myNewFeature: {
    id: 'my_new_feature',
    storageKey: 'kyomi_tour_my_new_feature',
    getSteps: (context) => [
      {
        element: '.my-feature-selector',  // CSS selector for element to highlight
        popover: {
          title: 'Cool New Feature 🎉',
          description: `
            <div style="line-height: 1.6;">
              <p style="margin-bottom: 12px;">Explanation of what this feature does.</p>
              <p><strong>Pro tip:</strong> How to use it effectively!</p>
            </div>
          `,
          side: 'bottom',  // top, bottom, left, right
          align: 'start'   // start, center, end
        }
      }
    ]
  }
};
```

### 2. Trigger the Tour in Your Component

```javascript
import { useProductTour } from '../components/ProductTour';

function MyFeatureComponent() {
  const { showTour } = useProductTour();

  useEffect(() => {
    // Show tour when component first mounts
    showTour('myNewFeature');
  }, []);

  // ... rest of component
}
```

### 3. Make Sure Element Has Correct Class

The element you target must exist in the DOM when the tour tries to show:

```jsx
<div className="my-feature-selector">
  {/* Your UI element here */}
</div>
```

## Examples

### Example 1: Show on Page Load

```javascript
// In SqlEditor.jsx
useEffect(() => {
  showTour('sqlEditor');
}, []);
```

### Example 2: Show After Action

```javascript
// In DashboardEditor.jsx
const handleSaveDashboard = async () => {
  await saveDashboard();
  // Show tour after first save
  showTour('dashboardEditor');
};
```

### Example 3: Show with Context Data

```javascript
// In Chat.jsx (already implemented)
if (fullContent && fullContent.includes('```chartml')) {
  showTour('firstChart', message.message_id);  // Pass messageId as context
}
```

## Testing Tours

**Note:** Tours are tracked in the backend database only. Use the reset script to clear tour state for testing.

### Reset Tours (Recommended)

Use the provided script:

```bash
./scripts/reset-tours.sh [email]
```

This resets your tour state in the database. Refresh your browser to see tours again.

### Reset Individual Tour (in code)

```javascript
const { resetTour } = useProductTour();
resetTour('firstChart');  // Clear locally only (temporary)
```

### Reset All Tours (in code)

```javascript
const { resetAllTours } = useProductTour();
resetAllTours();  // Clear locally only (temporary)
```

**Note:** The `resetTour()` and `resetAllTours()` functions only clear local React state, not the backend. Use the shell script for proper testing.

### Check Tour Progress

```javascript
import { getTourProgress } from '../components/ProductTour';

console.log(getTourProgress());
// { firstChart: true, sqlEditor: false, ... }
```

## Multi-Step Tours

You can add multiple steps to a single tour:

```javascript
myFeature: {
  id: 'my_feature',
  storageKey: 'kyomi_tour_my_feature',
  getSteps: () => [
    {
      element: '.step-1-element',
      popover: {
        title: 'Step 1',
        description: 'First thing to notice...'
      }
    },
    {
      element: '.step-2-element',
      popover: {
        title: 'Step 2',
        description: 'Then look at this...'
      }
    }
  ]
}
```

## Best Practices

1. **Keep it minimal** - Only tour truly important features
2. **Show at the right time** - Trigger when user first encounters the feature
3. **Make it skippable** - Users can always close tours
4. **Don't block work** - Tours enhance understanding, don't prevent action
5. **Use emojis sparingly** - Only for visual interest, not decoration
6. **Test element existence** - The tour system validates elements exist before showing

## Styling

Tours use Kyomi's design system automatically:
- Primary color (#d97706) for buttons
- Rounded corners and shadows
- Clean typography
- Semi-transparent dark overlay

Custom CSS is in `index.css` under "Product Tour Styles".

## When to Add a Tour

Add a tour when:
- ✅ Feature is non-obvious (e.g., star to pin)
- ✅ Feature is powerful but hidden (e.g., query history)
- ✅ Feature changes workflow (e.g., save chat as dashboard)

Don't add a tour when:
- ❌ Feature is self-explanatory (e.g., "Search" input)
- ❌ Feature is standard UI (e.g., "Delete" button)
- ❌ AI can explain it better (e.g., "How do I...?")

Remember: **The AI is the primary help system.** Tours are for quick wins.
