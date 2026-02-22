import { describe, it, expect, vi } from 'vitest'
import { mount } from '@vue/test-utils'
import Gallery from '../src/components/Gallery.vue'

vi.mock('vue-router', () => ({
  useRouter: () => ({ push: vi.fn() }),
}))

describe('Gallery.vue', () => {
  it('renders photos when fetched', async () => {
    const mockPhotos = [
      { id: '1', thumbnail_url: 'http://localhost:3000/1' },
      { id: '2', thumbnail_url: 'http://localhost:3000/2' }
    ]

    global.fetch = vi.fn().mockResolvedValue({
      ok: true,
      json: () => Promise.resolve(mockPhotos)
    })

    const wrapper = mount(Gallery)

    // Wait for onMounted and fetch
    await new Promise(resolve => setTimeout(resolve, 0))
    await wrapper.vm.$nextTick()

    const images = wrapper.findAll('img')
    expect(images.length).toBe(2)
    expect(images[0].attributes('src')).toBe('http://localhost:3000/1')
  })
})
