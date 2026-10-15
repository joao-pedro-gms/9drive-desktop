// Every icon the app draws, in one place.
//
// `lucide-svelte` 1.x ships legacy class components, so the shared prop type is Svelte's
// `ComponentType` rather than the Svelte 5 `Component`. Naming the icons here instead of
// importing each one where it is used keeps the icon set greppable and stops two components
// from picking two glyphs for the same action.

import type { ComponentType } from 'svelte';

export type IconComponent = ComponentType;

export {
  Archive,
  ArrowLeft,
  ArrowUp,
  Check,
  ChevronDown,
  ChevronRight,
  CircleAlert,
  Clock,
  Cloud,
  Database,
  Download,
  Ellipsis,
  EllipsisVertical,
  ExternalLink,
  Eye,
  File,
  FileText,
  FileUp,
  Folder,
  FolderOpen,
  FolderPlus,
  FolderSymlink,
  HardDrive,
  House,
  Image,
  Info,
  LayoutGrid,
  List,
  LoaderCircle,
  Moon,
  Music,
  PanelLeft,
  PenLine,
  Play,
  Plus,
  RefreshCw,
  Search,
  Settings,
  Square,
  Star,
  Sun,
  TriangleAlert,
  Trash2,
  Upload,
  Video,
  X,
} from 'lucide-svelte';