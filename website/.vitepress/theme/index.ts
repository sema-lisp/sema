import DefaultTheme from 'vitepress/theme'
import CustomHome from './CustomHome.vue'
import HomepageV2 from './HomepageV2.vue'
import BrandGuide from './BrandGuide.vue'
import IconShowcase from './IconShowcase.vue'
import CustomPageLayout from './CustomPageLayout.vue'
import SemaLogo from './SemaLogo.vue'
import FeatureNotebook from './FeatureNotebook.vue'
import FeatureAgents from './FeatureAgents.vue'
import FeatureCassettes from './FeatureCassettes.vue'
import FeatureObservability from './FeatureObservability.vue'
import FeatureBuild from './FeatureBuild.vue'
import FeatureExtraction from './FeatureExtraction.vue'
import FeatureEmbed from './FeatureEmbed.vue'
import FeatureRag from './FeatureRag.vue'
import FeatureWhatIsSema from './FeatureWhatIsSema.vue'
import FeatureWorkflows from './FeatureWorkflows.vue'
import HomeSearch from './HomeSearch.vue'
import './custom.css'

export default {
  extends: DefaultTheme,
  enhanceApp({ app }) {
    app.component('CustomHome', CustomHome)
    app.component('HomepageV2', HomepageV2)
    app.component('BrandGuide', BrandGuide)
    app.component('IconShowcase', IconShowcase)
    app.component('CustomPageLayout', CustomPageLayout)
    app.component('SemaLogo', SemaLogo)
    app.component('FeatureNotebook', FeatureNotebook)
    app.component('FeatureAgents', FeatureAgents)
    app.component('FeatureCassettes', FeatureCassettes)
    app.component('FeatureObservability', FeatureObservability)
    app.component('FeatureBuild', FeatureBuild)
    app.component('FeatureExtraction', FeatureExtraction)
    app.component('FeatureEmbed', FeatureEmbed)
    app.component('FeatureRag', FeatureRag)
    app.component('FeatureWhatIsSema', FeatureWhatIsSema)
    app.component('FeatureWorkflows', FeatureWorkflows)
    app.component('HomeSearch', HomeSearch)
  },
}
