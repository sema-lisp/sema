import DefaultTheme from 'vitepress/theme'
import CustomHome from './CustomHome.vue'
import HomepageV2 from './HomepageV2.vue'
import BrandGuide from './BrandGuide.vue'
import CustomPageLayout from './CustomPageLayout.vue'
import FeatureNotebook from './FeatureNotebook.vue'
import FeatureAgents from './FeatureAgents.vue'
import FeatureCassettes from './FeatureCassettes.vue'
import FeatureObservability from './FeatureObservability.vue'
import FeatureBuild from './FeatureBuild.vue'
import FeatureWhatIsSema from './FeatureWhatIsSema.vue'
import HomeSearch from './HomeSearch.vue'
import './custom.css'

export default {
  extends: DefaultTheme,
  enhanceApp({ app }) {
    app.component('CustomHome', CustomHome)
    app.component('HomepageV2', HomepageV2)
    app.component('BrandGuide', BrandGuide)
    app.component('CustomPageLayout', CustomPageLayout)
    app.component('FeatureNotebook', FeatureNotebook)
    app.component('FeatureAgents', FeatureAgents)
    app.component('FeatureCassettes', FeatureCassettes)
    app.component('FeatureObservability', FeatureObservability)
    app.component('FeatureBuild', FeatureBuild)
    app.component('FeatureWhatIsSema', FeatureWhatIsSema)
    app.component('HomeSearch', HomeSearch)
  },
}
