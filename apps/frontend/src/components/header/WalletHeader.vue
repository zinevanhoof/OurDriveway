<script setup lang="ts">
// The title WalletView used to render inline, lifted out so the wallet screen
// carries a header the same way the others do. Same size and weight it had; the
// `px-4 py-2` replaces the padding it inherited from the container it sat in.
import { Title } from '@/components/base/text';
</script>

<template>
    <Title as="h1" size="xl" weight="extrabold" class="px-4 py-2">
        Wallet
    </Title>
</template>
