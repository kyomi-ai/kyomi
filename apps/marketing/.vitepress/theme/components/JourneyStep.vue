<template>
  <div class="journey-step">
    <div class="journey-step-icon">
      <component :is="iconComponent" />
    </div>
    <div class="journey-step-content">
      <h3 class="journey-step-title">{{ title }}</h3>
      <p class="journey-step-description">
        <slot></slot>
      </p>
    </div>
  </div>
</template>

<script setup>
import { computed } from 'vue'
import {
  ChatBubbleLeftEllipsisIcon,
  MagnifyingGlassIcon,
  BookmarkIcon,
  PaintBrushIcon,
  ShareIcon,
  LightBulbIcon,
  SparklesIcon,
  ChartBarIcon,
  CogIcon,
  UserGroupIcon,
  BoltIcon,
  ShieldCheckIcon
} from '@heroicons/vue/24/outline'

const props = defineProps({
  icon: {
    type: String,
    required: true
  },
  title: {
    type: String,
    required: true
  }
})

// Map icon names to components
const iconMap = {
  ChatBubbleLeftEllipsisIcon,
  MagnifyingGlassIcon,
  BookmarkIcon,
  PaintBrushIcon,
  ShareIcon,
  LightBulbIcon,
  SparklesIcon,
  ChartBarIcon,
  CogIcon,
  UserGroupIcon,
  BoltIcon,
  ShieldCheckIcon
}

const iconComponent = computed(() => {
  return iconMap[props.icon] || ChatBubbleLeftEllipsisIcon
})
</script>

<style scoped>
.journey-step {
  display: flex;
  gap: 1.5rem;
  align-items: start;
  margin-bottom: 1.5rem;
  padding: 1.25rem;
  background: var(--color-muted);
  border-radius: 0.75rem;
  border-left: 3px solid var(--color-primary);
}

.journey-step-icon {
  width: 2.5rem;
  height: 2.5rem;
  flex-shrink: 0;
  color: var(--color-primary);
}

.journey-step-content {
  flex: 1;
}

.journey-step-title {
  font-size: 1.125rem;
  font-weight: 600;
  margin: 0 0 0.5rem 0;
}

.journey-step-description {
  color: var(--color-muted-foreground);
  margin: 0;
}
</style>
