// SPDX-License-Identifier: AGPL-3.0-or-later
import { createRoot } from 'react-dom/client'
import './index.css'
import App from './App.jsx'
// import './debug-capture.js' // Uncomment for layout debugging (Ctrl+Shift+C to capture)

createRoot(document.getElementById('root')).render(
  <App />
)
