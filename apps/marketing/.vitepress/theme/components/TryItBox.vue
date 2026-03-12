<template>
  <div class="try-it-box" :class="{ 'is-focused': isFocused }">
    <div class="try-it-header">
      <p class="try-it-label">Ask your first question</p>
      <p class="try-it-subtitle">Live demo on a sample SaaS database — no signup required</p>
    </div>

    <form class="search-form" @submit.prevent="submitCustomQuestion">
      <input
        v-model="customQuestion"
        type="text"
        class="search-input"
        placeholder="Ask a question about revenue, customers, churn..."
        @focus="isFocused = true"
        @blur="isFocused = false"
      />
      <button
        type="submit"
        class="search-button"
        :disabled="!customQuestion.trim()"
        aria-label="Submit question"
      >
        <ArrowRightIcon class="search-icon" />
      </button>
    </form>

    <div class="suggested-questions">
      <button
        v-for="question in suggestedQuestions"
        :key="question"
        class="question-chip"
        @click="handleChipClick(question)"
      >
        {{ question }}
      </button>
    </div>
  </div>
</template>

<script setup>
import { ref } from 'vue'
import { ArrowRightIcon } from '@heroicons/vue/24/solid'

const customQuestion = ref('')
const isFocused = ref(false)

const suggestedQuestions = [
  "What's our MRR trend?",
  "Top customers by revenue",
  "Churn rate by plan type",
  "Best converting pages"
]

// Use localhost in dev, production URL otherwise
const appUrl = typeof window !== 'undefined' && window.location.hostname === 'localhost'
  ? 'http://localhost:5173'
  : 'https://app.kyomi.ai'

function trackEvent(eventName, props) {
  if (typeof window !== 'undefined' && window.plausible) {
    window.plausible(eventName, { props })
  }
}

function goToTrial(question, source) {
  trackEvent('Trial Started', {
    source: source,
    question: question
  })
  const url = `${appUrl}/try?q=${encodeURIComponent(question)}`
  window.location.href = url
}

function handleChipClick(question) {
  goToTrial(question, 'suggested_chip')
}

function submitCustomQuestion() {
  if (customQuestion.value.trim()) {
    goToTrial(customQuestion.value.trim(), 'custom_input')
  }
}
</script>

<style scoped>
.try-it-box {
  width: 100%;
  max-width: 860px;
  margin: 2rem auto 0;
  padding: 1.75rem 2rem;
  text-align: center;
  background: linear-gradient(
    135deg,
    color-mix(in srgb, var(--color-primary) 10%, var(--color-background)) 0%,
    color-mix(in srgb, var(--color-primary) 5%, var(--color-background)) 100%
  );
  border: 2px solid color-mix(in srgb, var(--color-primary) 25%, var(--color-border));
  border-radius: 1rem;
  box-shadow: 0 4px 12px -2px color-mix(in srgb, var(--color-primary) 15%, transparent),
              0 2px 4px -1px rgba(0, 0, 0, 0.05);
  transition: box-shadow 0.2s ease, border-color 0.2s ease, transform 0.2s ease;
}

.try-it-box:hover {
  transform: translateY(-1px);
  box-shadow: 0 6px 16px -2px color-mix(in srgb, var(--color-primary) 20%, transparent),
              0 3px 6px -1px rgba(0, 0, 0, 0.06);
}

.try-it-box.is-focused {
  border-color: color-mix(in srgb, var(--color-primary) 60%, var(--color-border));
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--color-primary) 15%, transparent),
              0 4px 12px -2px color-mix(in srgb, var(--color-primary) 15%, transparent);
}

.try-it-header {
  margin-bottom: 1rem;
}

.try-it-label {
  font-size: 1.125rem;
  font-weight: 600;
  color: var(--color-foreground);
  margin: 0 0 0.25rem 0;
}

.try-it-subtitle {
  font-size: 0.875rem;
  color: var(--color-muted-foreground);
  margin: 0;
}

.search-form {
  display: flex;
  gap: 0;
  width: 100%;
  box-shadow: 0 2px 4px -1px rgba(0, 0, 0, 0.1), 0 1px 2px -1px rgba(0, 0, 0, 0.06);
  border-radius: 0.5rem;
  overflow: hidden;
}

.search-input {
  flex: 1;
  padding: 0.75rem 1rem;
  background: var(--color-background);
  border: 2px solid var(--color-border);
  border-right: none;
  border-radius: 0.5rem 0 0 0.5rem;
  font-size: 0.9375rem;
  color: var(--color-foreground);
  outline: none;
  transition: border-color 0.15s ease, box-shadow 0.15s ease;
}

.search-input:focus {
  border-color: var(--color-primary);
  box-shadow: 0 0 0 1px var(--color-primary);
}

.search-input::placeholder {
  color: var(--color-muted-foreground);
}

.search-button {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 3rem;
  background: var(--color-primary);
  color: white;
  border: 2px solid var(--color-primary);
  border-radius: 0 0.5rem 0.5rem 0;
  cursor: pointer;
  transition: background 0.15s ease, filter 0.15s ease;
}

.search-button:hover:not(:disabled) {
  filter: brightness(0.9);
}

.search-button:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.search-icon {
  width: 1.25rem;
  height: 1.25rem;
}

.suggested-questions {
  display: flex;
  flex-wrap: wrap;
  justify-content: center;
  gap: 0.5rem;
  margin-top: 0.75rem;
}

.question-chip {
  padding: 0.25rem 0.75rem;
  background: var(--color-background);
  border: 1px solid color-mix(in srgb, var(--color-primary) 25%, var(--color-border));
  border-radius: 9999px;
  font-size: 0.75rem;
  color: var(--color-muted-foreground);
  cursor: pointer;
  transition: all 0.15s ease;
  white-space: nowrap;
}

.question-chip:hover {
  background: var(--color-primary);
  color: white;
  border-color: var(--color-primary);
}

@media (max-width: 640px) {
  .try-it-box {
    padding: 1.25rem 1rem;
    border-radius: 0.75rem;
    margin: 1.5rem 1rem 0;
  }

  .try-it-label {
    font-size: 1rem;
  }

  .try-it-subtitle {
    font-size: 0.8125rem;
  }

  .search-input {
    padding: 0.625rem 0.875rem;
    font-size: 0.875rem;
  }

  .search-button {
    width: 2.75rem;
  }

  .question-chip {
    font-size: 0.75rem;
    padding: 0.375rem 0.75rem;
  }
}
</style>
