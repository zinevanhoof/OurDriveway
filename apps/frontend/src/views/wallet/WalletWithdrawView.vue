<script setup lang="ts">
import WalletWithdrawComponent from '@/components/wallet/WalletWithdrawComponent.vue';

const { maxWithdraw } = defineProps<{
    maxWithdraw: number
}>()
</script>

<template>
    <WalletWithdrawComponent :max-withdraw="maxWithdraw" />
</template>
