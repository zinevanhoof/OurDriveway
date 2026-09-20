<script setup lang="ts">
// Rendered by ProfileView itself, like every other header now — the route no longer
// carries one. `px-4 py-2` rather than the screen's own padding: this sits above
// ProfileView's container, and the pair adds up to the spacing the layout used to
// produce when the header hung outside `<main>`.
import { Title } from '@/components/base/text';
</script>

<template>
    <Title as="h1" size="xl" weight="extrabold" class="px-4 py-2">
        Profile
    </Title>
</template>
