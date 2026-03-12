<template>
  <div class="feature-card">
    <div class="feature-card-icon">
      <component :is="iconComponent" />
    </div>
    <h3 class="feature-card-title">{{ title }}</h3>
    <p class="feature-card-description">
      <slot></slot>
    </p>
  </div>
</template>

<script setup>
import { computed } from 'vue'
import {
  BoltIcon,
  CheckBadgeIcon,
  AdjustmentsHorizontalIcon,
  CurrencyDollarIcon,
  RocketLaunchIcon,
  ShieldCheckIcon,
  Cog6ToothIcon,
  SparklesIcon,
  StarIcon,
  CommandLineIcon,
  ChartBarIcon,
  UserGroupIcon,
  ChatBubbleLeftEllipsisIcon,
  BookmarkIcon,
  PaintBrushIcon,
  DocumentTextIcon,
  EyeIcon,
  LightBulbIcon,
  AcademicCapIcon,
  UserIcon,
  ArrowPathIcon,
  ShareIcon,
  Squares2X2Icon
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
  BoltIcon,
  CheckBadgeIcon,
  AdjustmentsHorizontalIcon,
  CurrencyDollarIcon,
  RocketLaunchIcon,
  ShieldCheckIcon,
  Cog6ToothIcon,
  SparklesIcon,
  StarIcon,
  CommandLineIcon,
  ChartBarIcon,
  UserGroupIcon,
  ChatBubbleLeftEllipsisIcon,
  BookmarkIcon,
  PaintBrushIcon,
  DocumentTextIcon,
  EyeIcon,
  LightBulbIcon,
  AcademicCapIcon,
  UserIcon,
  ArrowPathIcon,
  ShareIcon,
  Squares2X2Icon
}

const iconComponent = computed(() => {
  return iconMap[props.icon] || BoltIcon
})
</script>

<style scoped>
.feature-card {
  border: 1px solid var(--color-border);
  border-radius: 0.75rem;
  padding: 1.5rem;
  transition: all 0.2s;
  text-align: center;
}

.feature-card:hover {
  border-color: var(--color-primary);
  box-shadow: 0 4px 6px -1px rgb(0 0 0 / 0.1);
}

.feature-card-icon {
  width: 3rem;
  height: 3rem;
  margin: 0 auto 1rem;
  color: var(--color-primary);
}

.feature-card-title {
  font-size: 1.25rem;
  font-weight: 600;
  margin: 0 0 0.75rem 0;
}

.feature-card-description {
  color: var(--color-muted-foreground);
  margin: 0;
  line-height: 1.6;
}
</style>
