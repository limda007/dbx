<script setup lang="ts">
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";
import { AlertTriangle, Copy, RefreshCcw, X } from "@lucide/vue";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { copyToClipboard } from "@/lib/common/clipboard";
import { formatConnectionLifecycleDiagnostics } from "@/lib/connection/lifecycleClient";
import { useConnectionStore } from "@/stores/connectionStore";

const props = withDefaults(
  defineProps<{
    connectionId?: string | null;
    triggerClass?: string;
  }>(),
  {
    connectionId: null,
    triggerClass: "",
  },
);

const { t } = useI18n();
const connectionStore = useConnectionStore();
const copying = ref(false);
const reconnecting = ref(false);

const errorMessage = computed(() => (props.connectionId ? connectionStore.connectionErrors[props.connectionId] : ""));

const diagnostics = computed(() => {
  if (!props.connectionId) return null;
  return connectionStore.getConnectionLifecycleDiagnostics(props.connectionId);
});

function clearError() {
  if (props.connectionId) connectionStore.clearConnectionError(props.connectionId);
}

async function copyDiagnostics() {
  if (!props.connectionId || copying.value) return;
  copying.value = true;
  try {
    const snapshot = await connectionStore.loadConnectionLifecycleDiagnostics(props.connectionId);
    await copyToClipboard(formatConnectionLifecycleDiagnostics(snapshot));
  } finally {
    copying.value = false;
  }
}

async function forceReconnect() {
  if (!props.connectionId || reconnecting.value) return;
  reconnecting.value = true;
  try {
    await connectionStore.forceClearPoolsAndReconnect(props.connectionId);
  } catch {
    // Error is recorded on the connection; keep the indicator visible.
  } finally {
    reconnecting.value = false;
  }
}
</script>

<template>
  <Popover v-if="errorMessage">
    <PopoverTrigger as-child>
      <button type="button" class="inline-flex h-4 w-4 shrink-0 items-center justify-center rounded text-amber-500 hover:bg-amber-500/10 hover:text-amber-600 focus:outline-none focus:ring-1 focus:ring-amber-500/40" :class="triggerClass" :title="t('connection.lastError')" @click.stop>
        <AlertTriangle class="h-3.5 w-3.5" />
      </button>
    </PopoverTrigger>
    <PopoverContent side="top" class="w-72 gap-2 p-2 text-xs" @click.stop>
      <div class="flex items-start gap-2">
        <div class="min-w-0 flex-1">
          <div class="font-medium text-foreground">
            {{ t("connection.lastError") }}
          </div>
          <div class="mt-1 max-h-36 overflow-auto whitespace-pre-wrap break-words text-muted-foreground">
            {{ errorMessage }}
          </div>
          <div v-if="diagnostics" class="mt-1.5 text-[10px] leading-relaxed text-muted-foreground/80">
            <span v-if="diagnostics.dbType">{{ diagnostics.dbType }} · </span>
            <span>{{ diagnostics.connected ? t("connection.connected") : t("connection.disconnected") }}</span>
          </div>
        </div>
        <button type="button" class="shrink-0 rounded p-0.5 text-muted-foreground hover:bg-muted hover:text-foreground" :title="t('connection.clearError')" @click="clearError">
          <X class="h-3.5 w-3.5" />
        </button>
      </div>
      <button type="button" class="mt-1 inline-flex w-full items-center justify-center gap-1.5 rounded border border-border bg-background px-2 py-1 text-xs font-medium text-foreground hover:bg-muted disabled:opacity-60" :disabled="copying" @click="copyDiagnostics">
        <Copy class="h-3 w-3" :class="{ 'animate-pulse': copying }" />
        {{ t("connection.copyDiagnostics") }}
      </button>
      <button
        type="button"
        class="mt-1 inline-flex w-full items-center justify-center gap-1.5 rounded border border-border bg-background px-2 py-1 text-xs font-medium text-foreground hover:bg-muted disabled:opacity-60"
        :disabled="reconnecting"
        :title="t('connection.forceReconnectHint')"
        @click="forceReconnect"
      >
        <RefreshCcw class="h-3 w-3" :class="{ 'animate-spin': reconnecting }" />
        {{ reconnecting ? t("connection.forceReconnecting") : t("connection.forceReconnect") }}
      </button>
    </PopoverContent>
  </Popover>
</template>
