<template>
  <span class="inline-icon" :style="{ width: size, height: size }">
    <component :is="iconComponent" />
  </span>
</template>

<script setup>
import { computed } from 'vue'
import {
  ChartBarIcon,
  ArrowTrendingUpIcon,
  ChartBarSquareIcon,
  ChartPieIcon,
  TableCellsIcon,
  HashtagIcon,
  LinkIcon,
  SparklesIcon,
  CalendarDaysIcon,
  CircleStackIcon,
  PresentationChartBarIcon,
  RectangleGroupIcon,
  ViewColumnsIcon,
  CheckIcon
} from '@heroicons/vue/24/outline'

const props = defineProps({
  icon: {
    type: String,
    required: true
  },
  size: {
    type: String,
    default: '1.25em'
  }
})

// Map icon names to components
const iconMap = {
  ChartBarIcon,
  ArrowTrendingUpIcon,
  ChartBarSquareIcon,
  ChartPieIcon,
  TableCellsIcon,
  HashtagIcon,
  LinkIcon,
  SparklesIcon,
  CalendarDaysIcon,
  CircleStackIcon,
  PresentationChartBarIcon,
  RectangleGroupIcon,
  ViewColumnsIcon,
  CheckIcon
}

const iconComponent = computed(() => {
  return iconMap[props.icon] || ChartBarIcon
})
</script>

<style scoped>
.inline-icon {
  display: inline-flex;
  vertical-align: middle;
  color: var(--color-primary);
  margin-right: 0.25em;
}
</style>
