using System;
using System.Linq;
using System.Reflection;

var asm = Assembly.Load("Avalonia.Base");
var allTypes = asm.GetTypes();

void Dump(Type t)
{
    if (t == null) { Console.WriteLine("  (null)"); return; }
    Console.WriteLine("=== " + t.FullName + (t.IsInterface ? " [interface]" : " [class]") + " ===");
    foreach (var p in t.GetProperties(BindingFlags.Public | BindingFlags.Instance))
        Console.WriteLine("  prop  " + p.PropertyType.Name.PadRight(40) + " " + p.Name);
    foreach (var m in t.GetMethods(BindingFlags.Public | BindingFlags.Instance).Where(m => !m.IsSpecialName))
        Console.WriteLine("  fn    " + m.ReturnType.Name.PadRight(40) + " " + m.Name + "(" + string.Join(", ", m.GetParameters().Select(p => p.ParameterType.Name)) + ")");
    Console.WriteLine();
}

Console.WriteLine("### 在 Avalonia.Base 中搜索名字含 ImportedGpu 的类型 ###");
foreach (var t in allTypes.Where(t => t.Name.Contains("ImportedGpu") || t.Name == "CompositionSurface"))
    Console.WriteLine("  " + t.FullName);

Console.WriteLine();
Console.WriteLine("### ICompositionGpuImportedObject 全部成员 ###");
Dump(allTypes.FirstOrDefault(t => t.Name == "ICompositionGpuImportedObject"));

Console.WriteLine("### ICompositionImportedGpuImage 全部成员（含继承）###");
Dump(allTypes.FirstOrDefault(t => t.Name == "ICompositionImportedGpuImage"));

Console.WriteLine("### CompositionSurface 全部成员 ###");
Dump(allTypes.FirstOrDefault(t => t.Name == "CompositionSurface"));

Console.WriteLine("### CompositionDrawingSurface 全部成员 ###");
Dump(allTypes.FirstOrDefault(t => t.Name == "CompositionDrawingSurface"));
