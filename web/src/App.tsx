import { BrowserRouter } from 'react-router-dom'
import { WorkspaceProvider } from './app/WorkspaceProvider'
import { AppRouter } from './app/AppRouter'

export default function App() {
  return (
    <BrowserRouter>
      <WorkspaceProvider>
        <AppRouter />
      </WorkspaceProvider>
    </BrowserRouter>
  )
}
