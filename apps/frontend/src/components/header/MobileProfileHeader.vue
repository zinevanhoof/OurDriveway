<script setup lang="ts">
import { Settings } from '@lucide/vue';
</script>

<template>
    <header class="flex shrink-0 px-4 py-2 justify-between items-center bg-background">
        <div>
            <div class="text-2xl font-extrabold">Profile</div>
        </div>
        <div
            class="flex items-center justify-center border border-border w-10 h-10 rounded-full bg-card text-card-foreground">
            <settings :size="20" />
        </div>
    </header>
</template>