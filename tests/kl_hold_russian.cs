// Load the Russian keyboard layout (00000419) for the length of a test and
// unload it when the flag file named on the command line appears (issue #616).
// KLF_NOTELLSHELL keeps the shell's language bar out of it, and nothing is
// written to HKCU\Keyboard Layout\Preload. Prints the layout list before,
// during and after, and the virtual key the layout gives U+043D (it must be
// VK_Y, 0x59), so the test can check the machine is left as it was.
using System;
using System.IO;
using System.Runtime.InteropServices;
using System.Threading;

class KlHoldRussian {
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern IntPtr LoadKeyboardLayoutW(string id, uint flags);
    [DllImport("user32.dll")] static extern bool UnloadKeyboardLayout(IntPtr hkl);
    [DllImport("user32.dll")] static extern int GetKeyboardLayoutList(int n, IntPtr[] list);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern short VkKeyScanExW(char ch, IntPtr hkl);

    static string List() {
        int n = GetKeyboardLayoutList(0, null);
        var a = new IntPtr[n];
        GetKeyboardLayoutList(n, a);
        var s = new string[n];
        for (int i = 0; i < n; i++) s[i] = a[i].ToInt64().ToString("X");
        return string.Join(",", s);
    }

    static int Main(string[] args) {
        string flag = args[0];
        string before = List();
        Console.WriteLine("before=" + before);
        IntPtr hkl = LoadKeyboardLayoutW("00000419", 0x80 /* KLF_NOTELLSHELL */);
        // Only unload what this process added: a layout the user already had
        // stays loaded.
        bool added = before.IndexOf(hkl.ToInt64().ToString("X"), StringComparison.Ordinal) < 0;
        Console.WriteLine("loaded=" + hkl.ToInt64().ToString("X") + " added=" + added);
        Console.WriteLine("vk=0x" + (VkKeyScanExW('н', hkl) & 0xFF).ToString("X2"));
        Console.WriteLine("during=" + List());
        Console.Out.Flush();
        try {
            for (int i = 0; i < 1200 && !File.Exists(flag); i++) Thread.Sleep(100);
        } finally {
            if (added) Console.WriteLine("unloaded=" + UnloadKeyboardLayout(hkl));
            Console.WriteLine("after=" + List());
        }
        return 0;
    }
}
