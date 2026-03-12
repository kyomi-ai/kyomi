// Custom theme for Kyomi marketing site
// Extends default VitePress theme with custom styles matching app design system

import DefaultTheme from 'vitepress/theme'
import { h } from 'vue'
import './style.css'
import JourneyStep from './components/JourneyStep.vue'
import FeatureCard from './components/FeatureCard.vue'
import InlineIcon from './components/InlineIcon.vue'
import TryItBox from './components/TryItBox.vue'

export default {
  extends: DefaultTheme,
  Layout() {
    return h(DefaultTheme.Layout, null, {
      // Inject TryItBox into the hero section, after the tagline (replaces action buttons)
      'home-hero-info-after': () => h(TryItBox)
    })
  },
  enhanceApp({ app, router, siteData }) {
    // Register global components
    app.component('JourneyStep', JourneyStep)
    app.component('FeatureCard', FeatureCard)
    app.component('InlineIcon', InlineIcon)
    app.component('TryItBox', TryItBox)
  }
}
