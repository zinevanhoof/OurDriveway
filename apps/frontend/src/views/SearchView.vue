<script setup lang="ts">
import LocationMap from '@/components/map/LocationMap.vue';
</script>

<template>
  <!-- Hasselt, Grote Markt — until the user's own location comes in. -->
  <LocationMap class="h-full w-full" :lng="5.3378" :lat="50.9306" :zoom="13" />
</template>
