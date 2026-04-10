// Custom theme for Kyomi marketing site
// Extends default VitePress theme with custom styles matching app design system

import DefaultTheme from 'vitepress/theme'
import './style.css'
import JourneyStep from './components/JourneyStep.vue'
import FeatureCard from './components/FeatureCard.vue'
import InlineIcon from './components/InlineIcon.vue'
export default {
  extends: DefaultTheme,
  enhanceApp({ app, router, siteData }) {
    // Register global components
    app.component('JourneyStep', JourneyStep)
    app.component('FeatureCard', FeatureCard)
    app.component('InlineIcon', InlineIcon)
  }
}
