<script setup lang="ts">
import {
    Tabs,
    TabsContent,
    TabsList,
    TabsTrigger,
} from '@/components/ui/tabs'
import LoginForm from './forms/login-signup-form/LoginForm.vue'
import SignupForm from './forms/login-signup-form/SignupForm.vue'
import { AuthResponse } from '@/types/response/AuthResponse.ts'
import { useAuthStore } from '@/stores/auth.ts'
import { ref } from 'vue'
import { useRouter } from 'vue-router'
import { toast } from 'vue-sonner'
import { fetchMe } from '@/api/me.ts'

const auth = useAuthStore()
const router = useRouter()

const tab = ref<string>('login')

const login = async (response: AuthResponse) => {
    auth.setAccessToken(response.access_token)
    const user = await fetchMe()
    auth.setUser(user)
    router.push({ name: 'home' })
}

// No session is issued at signup and login will refuse until the address is
// confirmed, so this has to say what happens next rather than "you're in".
const signup = () => {
    tab.value = 'login'
    toast.success('Check your email to verify your address, then log in.')
}
</script>

<template>
    <div class="mx-4 mt-22">
        <Tabs v-model="tab">
            <TabsList class="w-full">
                <TabsTrigger value="login">
                    Login
                </TabsTrigger>
                <TabsTrigger value="signup">
                    Signup
                </TabsTrigger>
            </TabsList>
            <TabsContent value="login">
                <LoginForm @success="login" />
            </TabsContent>
            <TabsContent value="signup">
                <SignupForm @success="signup" />
            </TabsContent>
        </Tabs>
    </div>
</template>
