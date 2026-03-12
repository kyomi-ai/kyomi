// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Debug capture tool - Press Ctrl+Shift+C to capture full DOM state
 * This will save a JSON file with ALL computed styles and positions
 */

let captureCount = 0;

function captureFullState() {
  const capture = {
    timestamp: new Date().toISOString(),
    url: window.location.href,
    viewport: {
      width: window.innerWidth,
      height: window.innerHeight,
    },
    elements: {}
  };

  // Capture ALL divs and their computed styles
  const allDivs = document.querySelectorAll('div, h1, h2, button');

  allDivs.forEach((el, idx) => {
    const rect = el.getBoundingClientRect();
    const styles = window.getComputedStyle(el);
    const classes = el.className;

    // Create a unique identifier
    const id = el.id || `${el.tagName}-${idx}-${classes.substring(0, 30)}`;

    capture.elements[id] = {
      tag: el.tagName,
      classes: classes,
      inlineStyle: el.style.cssText,
      rect: {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height
      },
      computed: {
        display: styles.display,
        position: styles.position,
        flex: styles.flex,
        flexDirection: styles.flexDirection,
        overflow: styles.overflow,
        margin: styles.margin,
        padding: styles.padding,
        height: styles.height,
        width: styles.width,
      },
      text: el.textContent?.substring(0, 50) || ''
    };
  });

  // Download as JSON
  const blob = new Blob([JSON.stringify(capture, null, 2)], { type: 'application/json' });
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = `layout-capture-${captureCount++}-${capture.timestamp}.json`;
  a.click();

  alert(`Captured! Files saved. Capture #${captureCount}.\n\nNow refresh until you see the broken layout, then press Ctrl+Shift+C again.`);
}

// Add keyboard listener
document.addEventListener('keydown', (e) => {
  if (e.ctrlKey && e.shiftKey && e.key === 'C') {
    e.preventDefault();
    captureFullState();
  }
});

