import { Architecture } from '@/components/architecture'
import { Devices } from '@/components/devices'
import { Features } from '@/components/features'
import { Footer } from '@/components/footer'
import { GetStarted } from '@/components/get-started'
import { Hero } from '@/components/hero'
import { Nav } from '@/components/nav'
import { SDKPreview } from '@/components/sdk-preview'
import { Showcase } from '@/components/showcase'

export default function Home() {
  return (
    <>
      <Nav />
      <main>
        <Hero />
        <Features />
        <Showcase />
        <SDKPreview />
        <Devices />
        <Architecture />
        <GetStarted />
      </main>
      <Footer />
    </>
  )
}
